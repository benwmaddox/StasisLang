use serde_json::{json, Value};
use stasis_compiler::backend::jit::{JitEnginePackage, JitProcess, JitScalarValue, JitStateLayout};
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use stasis_compiler::backend::EngineEntrypoints;
use stasis_compiler::compiler::CompileError;
use stasis_compiler::frontend::workshop::{
    find_workshop_references, find_workshop_symbols, load_workshop_edit_workspace,
    plan_workshop_semantic_edits, workshop_completion_items, workshop_direct_import_files,
    workshop_reachable_files, workshop_source_hash, workshop_source_items,
    write_workshop_semantic_plan, write_workshop_semantic_receipt, ExpectedReload,
    WorkshopCompletionItem, WorkshopSemanticEdit, WorkshopSemanticEditBatch,
    WorkshopSemanticEditOperation, WorkshopSemanticEditPlan, WorkshopSourceFile,
    WorkshopSourceItem, WorkshopSourceItemKind, WorkshopSymbolSelector,
};
use stasis_language_service::LanguageCompletionSnapshot;
use stasis_runner::live::{
    compare_live_validation_values, CompletionContext, CompletionIndex, CompletionItem,
    CompletionQuery, CompletionScope, LiveCommand, LiveEditOperation, LiveRequest, LiveResponse,
    LiveResponseSendError, LiveSessionServer, LiveSymbolTarget, ScratchWorkspace, MAX_LIVE_WATCHES,
};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

use stasis_compiler::backend::state_migration::{
    activate_candidate_transactionally, finalize_runtime_preview, plan_state_migration,
    state_layout_version, StateMigrationPreview as LiveSwapPreview,
};

const REQUESTS_PER_TICK: usize = 8;
const MAX_PENDING_LIVE_REQUESTS: usize = 64;
const MAX_LIVE_EDIT_SOURCE_BYTES: usize = 256 * 1024;
const MAX_LIVE_EDIT_BATCH: usize = 64;
const MAX_LIVE_TRANSACTION_ASSIGNMENTS: usize = 64;
const MAX_STAGED_TEST_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_STAGED_TEST_DIAGNOSTIC_BYTES: usize = 4096;
const MAX_WATCH_PREDICATE_SCAN_PER_TICK: usize = 4096;
#[derive(Debug, Clone)]
pub struct LiveRunConfig {
    pub project_root: PathBuf,
    pub entry: PathBuf,
    pub output: PathBuf,
    pub window_title: Option<String>,
}

impl LiveRunConfig {
    pub fn new(project_root: PathBuf, entry: PathBuf, output: PathBuf) -> Self {
        Self {
            project_root,
            entry,
            output,
            window_title: None,
        }
    }

    pub fn with_window_title(mut self, window_title: impl Into<String>) -> Self {
        self.window_title = Some(window_title.into());
        self
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    plan: WorkshopSemanticEditPlan,
    swap_preview: LiveSwapPreview,
    receipt: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct PendingEdit {
    plan: WorkshopSemanticEditPlan,
    swap_preview: LiveSwapPreview,
}

#[derive(Debug, Clone)]
enum PreparedAction {
    Preview,
    ApplyNew,
    ApplyPending,
    Undo { index: usize },
    Redo { index: usize },
}

struct PreparedEdit {
    request_id: u64,
    plan: WorkshopSemanticEditPlan,
    swap_preview: LiveSwapPreview,
    expected_swap_preview: Option<LiveSwapPreview>,
    restore: bool,
    action: PreparedAction,
    tests_ran: bool,
    candidate: JitProcess,
    package: JitEnginePackage,
    source_items: Vec<WorkshopSourceItem>,
    completion_items: Vec<WorkshopCompletionItem>,
    source_files: Vec<WorkshopSourceFile>,
    input_hashes: BTreeMap<String, String>,
}

struct EditPreparation {
    request_id: u64,
    canceled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<PreparedEdit, String>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

struct CompletionPreparation {
    request_id: u64,
    receiver: mpsc::Receiver<CompletionQuery>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[derive(Clone, Default)]
struct CompletionSnapshot {
    index: CompletionIndex,
    language: LanguageCompletionSnapshot,
}

#[derive(Debug, Clone)]
enum EditPreparationInput {
    Edit {
        operation: LiveEditOperation,
        target: LiveSymbolTarget,
        source: Option<String>,
        expected_source_hash: Option<String>,
        preview: bool,
        run_tests: bool,
    },
    EditBatch {
        edits: Vec<stasis_runner::live::LiveEdit>,
        preview: bool,
        run_tests: bool,
    },
    Plan {
        plan: WorkshopSemanticEditPlan,
        expected_swap_preview: Option<LiveSwapPreview>,
        restore: bool,
        run_tests: bool,
        action: PreparedAction,
    },
    Persist {
        target: LiveSymbolTarget,
        source: String,
        preview: bool,
        run_tests: bool,
    },
}

pub(crate) struct LiveWorkspace {
    server: LiveSessionServer,
    config: LiveRunConfig,
    paused: bool,
    step_remaining: u32,
    quit: bool,
    history: Vec<HistoryEntry>,
    history_cursor: usize,
    completion: CompletionIndex,
    source_items: Vec<WorkshopSourceItem>,
    completion_items: Vec<WorkshopCompletionItem>,
    source_files: Vec<WorkshopSourceFile>,
    completion_snapshot: Arc<CompletionSnapshot>,
    scratch: ScratchWorkspace,
    watches: BTreeMap<String, Option<Value>>,
    pending_plan: Option<PendingEdit>,
    pending_requests: VecDeque<LiveRequest>,
    pending_responses: VecDeque<LiveResponse>,
    self_write_hashes: BTreeMap<PathBuf, String>,
    edit_preparation: Option<EditPreparation>,
    completion_preparation: Option<CompletionPreparation>,
    dropped_watch_events: u64,
    validation_snapshot: Option<stasis_dynload::JitRuntimeStateSnapshot>,
    host_entry_revision: u64,
}

impl Drop for LiveWorkspace {
    fn drop(&mut self) {
        if let Some(mut preparation) = self.edit_preparation.take() {
            preparation.canceled.store(true, Ordering::Release);
            if let Some(worker) = preparation.worker.take() {
                let _ = worker.join();
            }
        }
        if let Some(mut preparation) = self.completion_preparation.take() {
            if let Some(worker) = preparation.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

impl LiveWorkspace {
    pub(crate) fn new(
        server: LiveSessionServer,
        config: LiveRunConfig,
        jit: &JitProcess,
    ) -> Result<Self, String> {
        let mut workspace = Self {
            server,
            config,
            paused: false,
            step_remaining: 0,
            quit: false,
            history: Vec::new(),
            history_cursor: 0,
            completion: CompletionIndex::default(),
            source_items: Vec::new(),
            completion_items: Vec::new(),
            source_files: Vec::new(),
            completion_snapshot: Arc::new(CompletionSnapshot::default()),
            scratch: ScratchWorkspace::default(),
            watches: BTreeMap::new(),
            pending_plan: None,
            pending_requests: VecDeque::new(),
            pending_responses: VecDeque::new(),
            self_write_hashes: BTreeMap::new(),
            edit_preparation: None,
            completion_preparation: None,
            dropped_watch_events: 0,
            validation_snapshot: None,
            host_entry_revision: stasis_dynload::jit_host_entry_targets()
                .map_or(0, |targets| targets.revision),
        };
        workspace.refresh_completion(jit)?;
        Ok(workspace)
    }

    pub(crate) fn process_boundary(
        &mut self,
        tick: u64,
        jit: &mut JitProcess,
        tick_code_ptr: &mut u64,
        render_code_ptr: &mut u64,
    ) {
        while let Some(response) = self.pending_responses.pop_front() {
            match self.server.respond(response) {
                Ok(()) => {}
                Err(LiveResponseSendError::Full(response)) => {
                    self.pending_responses.push_front(response);
                    return;
                }
                Err(LiveResponseSendError::Disconnected) => {
                    self.quit = true;
                    return;
                }
            }
        }
        let incoming = self.server.drain(usize::MAX);
        let cancellation_targets = incoming
            .iter()
            .filter_map(|request| match request.command {
                LiveCommand::Cancel { request_id } => Some(request_id),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let mut background_canceled_targets = BTreeSet::new();
        for canceled in &cancellation_targets {
            if let Some(preparation) = self
                .edit_preparation
                .as_ref()
                .filter(|preparation| preparation.request_id == *canceled)
            {
                preparation.canceled.store(true, Ordering::Release);
                background_canceled_targets.insert(*canceled);
            }
        }
        let quit_queued = incoming
            .iter()
            .any(|request| matches!(request.command, LiveCommand::Quit));
        if quit_queued {
            if let Some(preparation) = self.edit_preparation.as_ref() {
                preparation.canceled.store(true, Ordering::Release);
            }
        }
        let mut controls = Vec::new();
        for request in incoming {
            if matches!(
                request.command,
                LiveCommand::Cancel { .. } | LiveCommand::Quit
            ) {
                controls.push(request);
            } else if self.pending_requests.len() < MAX_PENDING_LIVE_REQUESTS {
                self.pending_requests.push_back(request);
            } else {
                self.enqueue_response(LiveResponse::failure(
                    request.request_id,
                    tick,
                    format!(
                        "live request backlog exceeds {MAX_PENDING_LIVE_REQUESTS}; retry after backpressure clears"
                    ),
                ));
            }
        }
        for canceled in &cancellation_targets {
            if let Some(index) = self
                .pending_requests
                .iter()
                .position(|request| request.request_id == *canceled)
            {
                let request = self
                    .pending_requests
                    .remove(index)
                    .expect("pending cancellation index exists");
                self.enqueue_response(LiveResponse::failure(
                    request.request_id,
                    tick,
                    "live request canceled before boundary execution",
                ));
            }
        }
        if let Some(response) =
            self.finish_edit_preparation(tick, jit, tick_code_ptr, render_code_ptr)
        {
            self.enqueue_response(response);
        }
        if let Some(response) = self.finish_completion_preparation(tick) {
            self.enqueue_response(response);
        }
        for request in controls {
            let response = match request.command {
                LiveCommand::Cancel {
                    request_id: canceled,
                } => LiveResponse::success(
                    request.request_id,
                    tick,
                    "cancellation_requested",
                    json!({"request_id": canceled, "background": background_canceled_targets.contains(&canceled)}),
                ),
                LiveCommand::Quit => {
                    self.quit = true;
                    LiveResponse::success(
                        request.request_id,
                        tick,
                        "quitting",
                        json!({"tick": tick}),
                    )
                }
                _ => unreachable!("only controls are separated"),
            };
            self.enqueue_response(response);
        }
        if self.quit {
            return;
        }
        for _ in 0..REQUESTS_PER_TICK {
            let Some(request) = self.pending_requests.pop_front() else {
                break;
            };
            let request_id = request.request_id;
            let response = match request.validate() {
                Ok(()) => self.handle_request(request, tick, jit),
                Err(error) => LiveResponse::failure(request_id, tick, error),
            };
            self.enqueue_response(response);
            if self.quit {
                break;
            }
        }
    }

    fn enqueue_response(&mut self, response: LiveResponse) {
        match self.server.respond(response) {
            Ok(()) => {}
            Err(LiveResponseSendError::Full(response)) => {
                self.pending_responses.push_back(response);
            }
            Err(LiveResponseSendError::Disconnected) => self.quit = true,
        }
    }

    pub(crate) fn should_run_tick(&self) -> bool {
        !self.paused || self.step_remaining > 0
    }

    pub(crate) fn after_tick(&mut self) {
        if self.paused && self.step_remaining > 0 {
            self.step_remaining -= 1;
        }
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.quit
    }

    pub(crate) fn refresh_after_external_edit(&mut self, jit: &JitProcess) {
        self.sync_host_entry_revision();
        let _ = self.refresh_completion(jit);
    }

    fn sync_host_entry_revision(&mut self) {
        if let Some(targets) = stasis_dynload::jit_host_entry_targets() {
            self.host_entry_revision = self.host_entry_revision.max(targets.revision);
        }
    }

    pub(crate) fn consumes_self_write(&mut self, path: &Path) -> bool {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let Some(expected_hash) = self.self_write_hashes.get(&path) else {
            return false;
        };
        let matches = std::fs::read_to_string(&path)
            .is_ok_and(|source| workshop_source_hash(&source) == *expected_hash);
        if !matches {
            self.self_write_hashes.remove(&path);
        }
        matches
    }

    pub(crate) fn publish_watches(&mut self, tick: u64, jit: &JitProcess) {
        if self.dropped_watch_events > 0 {
            let dropped = self.dropped_watch_events;
            match self.server.respond(LiveResponse::success(
                0,
                tick,
                "watch_backpressure",
                json!({"dropped_events": dropped}),
            )) {
                Ok(()) => self.dropped_watch_events = 0,
                Err(LiveResponseSendError::Full(_)) => return,
                Err(LiveResponseSendError::Disconnected) => {
                    self.quit = true;
                    return;
                }
            }
        }
        let paths = self.watches.keys().cloned().collect::<Vec<_>>();
        let mut remaining_scan = MAX_WATCH_PREDICATE_SCAN_PER_TICK;
        let path_count = paths.len();
        for (index, path) in paths.into_iter().enumerate() {
            let paths_left = path_count - index;
            let scan_limit = if remaining_scan == 0 {
                0
            } else {
                (remaining_scan / paths_left).max(1)
            };
            let value = match jit.inspect_state_query_with_scan_limit(&path, scan_limit) {
                Ok(value) => {
                    let scanned = value
                        .get("scanned")
                        .and_then(Value::as_u64)
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(0);
                    remaining_scan = remaining_scan.saturating_sub(scanned);
                    value
                }
                Err(error) => {
                    let value = json!({"kind": "error", "error": error.clone()});
                    let prior = self.watches.get(&path).and_then(Option::as_ref);
                    if prior == Some(&value) {
                        continue;
                    }
                    self.watches.insert(path.clone(), Some(value));
                    match self.server.respond(LiveResponse::success(
                        0,
                        tick,
                        "watch_error",
                        json!({"path": path, "error": error}),
                    )) {
                        Ok(()) => {}
                        Err(LiveResponseSendError::Full(_)) => {
                            self.dropped_watch_events = self.dropped_watch_events.saturating_add(1);
                        }
                        Err(LiveResponseSendError::Disconnected) => {
                            self.quit = true;
                            return;
                        }
                    }
                    continue;
                }
            };
            let prior = self.watches.get(&path).and_then(Option::as_ref);
            if prior == Some(&value) {
                continue;
            }
            self.watches.insert(path.clone(), Some(value.clone()));
            let observed = inspection_observed_value(&value);
            match self.server.respond(LiveResponse::success(
                0,
                tick,
                "watch",
                json!({"path": path, "value": observed, "inspection": value}),
            )) {
                Ok(()) => {}
                Err(LiveResponseSendError::Full(_)) => {
                    self.dropped_watch_events = self.dropped_watch_events.saturating_add(1);
                }
                Err(LiveResponseSendError::Disconnected) => {
                    self.quit = true;
                    return;
                }
            }
        }
    }

    fn handle_request(
        &mut self,
        request: LiveRequest,
        tick: u64,
        jit: &mut JitProcess,
    ) -> LiveResponse {
        let request_id = request.request_id;
        let result: Result<(&'static str, Value), String> = (|| match request.command {
            LiveCommand::Help => Ok(("help", help_data())),
            LiveCommand::Status => Ok((
                "status",
                json!({
                    "paused": self.paused,
                    "step_remaining": self.step_remaining,
                    "tick": tick,
                    "history_length": self.history.len(),
                    "history_cursor": self.history_cursor,
                    "watches": self.watches.keys().collect::<Vec<_>>(),
                    "scratch_cells": self.scratch.list(),
                    "dropped_watch_events": self.dropped_watch_events,
                    "preparing_request_id": self.edit_preparation.as_ref().map(|job| job.request_id),
                }),
            )),
            LiveCommand::Pause => {
                self.paused = true;
                self.step_remaining = 0;
                Ok(("paused", json!({"paused": true, "tick": tick})))
            }
            LiveCommand::Resume => {
                self.paused = false;
                self.step_remaining = 0;
                Ok(("resumed", json!({"paused": false, "tick": tick})))
            }
            LiveCommand::Step { ticks } if ticks == 0 => {
                Err("step ticks must be greater than zero".to_string())
            }
            LiveCommand::Step { ticks } => {
                self.paused = true;
                self.step_remaining = ticks;
                Ok((
                    "step_scheduled",
                    json!({"ticks": ticks, "after_tick": tick}),
                ))
            }
            LiveCommand::Cancel { .. } => unreachable!("cancellation handled before dispatch"),
            LiveCommand::Quit => {
                if let Some(preparation) = self.edit_preparation.as_ref() {
                    preparation.canceled.store(true, Ordering::Release);
                }
                self.quit = true;
                Ok(("quitting", json!({"tick": tick})))
            }
            LiveCommand::Symbols {
                query,
                kind,
                files,
                owner,
                page,
                limit,
            } => self.symbols(
                query.as_deref(),
                kind.as_deref(),
                &files,
                owner.as_deref(),
                page,
                limit,
            ),
            LiveCommand::Read {
                name,
                kind,
                file,
                owner,
                signature,
            } => self.read_symbol(LiveSymbolTarget {
                name,
                kind,
                file,
                owner,
                signature,
            }),
            LiveCommand::References { symbol, limit } => Ok((
                "references",
                json!({
                    "symbol": symbol,
                    "references": find_workshop_references(&self.source_files, &symbol, limit)?,
                }),
            )),
            LiveCommand::Validate {
                requirement,
                frames,
            } => validate_live_runtime(jit, &requirement, frames),
            LiveCommand::ValidationSnapshot => {
                self.validation_snapshot = Some(
                    stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES)?,
                );
                Ok(("validation_snapshot", json!({"captured": true})))
            }
            LiveCommand::ValidationRestore => {
                let snapshot = self
                    .validation_snapshot
                    .as_ref()
                    .ok_or_else(|| "no AI validation baseline is available".to_string())?;
                stasis_dynload::restore_jit_runtime_state(snapshot);
                Ok(("validation_restored", json!({"restored": true})))
            }
            LiveCommand::ValidationClear => {
                self.validation_snapshot = None;
                Ok(("validation_cleared", json!({"cleared": true})))
            }
            LiveCommand::Complete {
                buffer,
                cursor,
                limit,
                context,
            } => self.start_completion_preparation(request_id, buffer, cursor, limit, context),
            LiveCommand::Palette {
                query,
                page,
                limit,
                context,
            } => Ok((
                "palette",
                json!(paged_completion_query(
                    &self.completion,
                    &query,
                    page,
                    limit,
                    &context,
                )),
            )),
            LiveCommand::Edit {
                operation,
                target,
                source,
                expected_source_hash,
                preview,
                run_tests,
            } => self.start_edit_preparation(
                request_id,
                EditPreparationInput::Edit {
                    operation,
                    target,
                    source,
                    expected_source_hash,
                    preview,
                    run_tests,
                },
                jit,
            ),
            LiveCommand::EditBatch {
                edits,
                preview,
                run_tests,
            } => self.start_edit_preparation(
                request_id,
                EditPreparationInput::EditBatch {
                    edits,
                    preview,
                    run_tests,
                },
                jit,
            ),
            LiveCommand::Preview => self
                .pending_plan
                .as_ref()
                .map(|pending| {
                    (
                        "edit_preview",
                        json!({
                            "validated": pending.swap_preview.state_layout_compatible,
                            "plan": pending.plan,
                            "swap": pending.swap_preview,
                        }),
                    )
                })
                .ok_or_else(|| "no validated live semantic preview is pending".to_string()),
            LiveCommand::Apply { run_tests } => {
                let pending = self
                    .pending_plan
                    .clone()
                    .ok_or_else(|| "no validated live semantic preview is pending".to_string())?;
                self.start_edit_preparation(
                    request_id,
                    EditPreparationInput::Plan {
                        plan: pending.plan,
                        expected_swap_preview: Some(pending.swap_preview),
                        restore: false,
                        run_tests,
                        action: PreparedAction::ApplyPending,
                    },
                    jit,
                )
            }
            LiveCommand::Changes => Ok((
                "changes",
                json!({
                    "cursor": self.history_cursor,
                    "entries": self.history.iter().enumerate().map(|(index, entry)| json!({
                        "index": index,
                        "applied": index < self.history_cursor,
                        "receipt": entry.receipt,
                        "changed_files": entry.plan.changed_files.iter().map(|change| &change.file).collect::<Vec<_>>(),
                        "changed_symbols": entry.plan.reload.changed_symbols,
                        "swap": entry.swap_preview,
                    })).collect::<Vec<_>>()
                }),
            )),
            LiveCommand::Undo { run_tests } => {
                let index = self
                    .history_cursor
                    .checked_sub(1)
                    .ok_or_else(|| "no live semantic edit to undo".to_string())?;
                self.start_edit_preparation(
                    request_id,
                    EditPreparationInput::Plan {
                        plan: self.history[index].plan.clone(),
                        expected_swap_preview: None,
                        restore: true,
                        run_tests,
                        action: PreparedAction::Undo { index },
                    },
                    jit,
                )
            }
            LiveCommand::Redo { run_tests } => {
                if self.history_cursor >= self.history.len() {
                    Err("no live semantic edit to redo".to_string())
                } else {
                    let index = self.history_cursor;
                    self.start_edit_preparation(
                        request_id,
                        EditPreparationInput::Plan {
                            plan: self.history[index].plan.clone(),
                            expected_swap_preview: None,
                            restore: false,
                            run_tests,
                            action: PreparedAction::Redo { index },
                        },
                        jit,
                    )
                }
            }
            LiveCommand::Inspect { path } => inspect_scalar(jit, &path),
            LiveCommand::InspectAll { limit, concise } => inspect_all_scalars(jit, limit, concise),
            LiveCommand::Watch { path } => {
                let value = jit.inspect_state_query(&path)?;
                if !self.watches.contains_key(&path) && self.watches.len() >= MAX_LIVE_WATCHES {
                    return Err(format!(
                        "live watching is limited to {MAX_LIVE_WATCHES} paths"
                    ));
                }
                let observed = inspection_observed_value(&value);
                self.watches.insert(path.clone(), Some(value));
                Ok((
                    "watch_added",
                    json!({"path": path, "value": observed, "tick": tick}),
                ))
            }
            LiveCommand::Unwatch { path } => {
                if let Some(path) = path {
                    self.watches.remove(&path);
                } else {
                    self.watches.clear();
                }
                Ok((
                    "watch_removed",
                    json!({"watches": self.watches.keys().collect::<Vec<_>>()}),
                ))
            }
            LiveCommand::Set {
                path,
                expression,
                preview,
            } => set_scalar(jit, &path, &expression, preview),
            LiveCommand::Print { expression } => print_scalar(jit, &expression),
            LiveCommand::Evaluate { expression } => evaluate_expression(jit, &expression),
            LiveCommand::Do { code, preview } => apply_scalar_transaction(jit, &code, preview),
            LiveCommand::CellPut { name, code } => {
                self.scratch.put(&name, code)?;
                self.rebuild_completion(jit);
                Ok(("cell_saved", json!({"name": name, "persistent": false})))
            }
            LiveCommand::CellRun { name, preview } => {
                let code = self
                    .scratch
                    .get(&name)
                    .ok_or_else(|| format!("scratch cell '{name}' not found"))?
                    .code
                    .clone();
                let (kind, data) = apply_scalar_transaction(jit, &code, preview)?;
                self.scratch.record_result(&name, tick, data.to_string())?;
                Ok((
                    kind,
                    json!({"name": name, "result": data, "persistent": false}),
                ))
            }
            LiveCommand::CellList => Ok(("cells", json!({"cells": self.scratch.list()}))),
            LiveCommand::CellClear { name } => {
                self.scratch.clear(name.as_deref())?;
                self.rebuild_completion(jit);
                Ok(("cells_cleared", json!({"name": name})))
            }
            LiveCommand::CellPersist {
                name,
                target,
                preview,
                run_tests,
            } => {
                let source = self
                    .scratch
                    .get(&name)
                    .ok_or_else(|| format!("scratch cell '{name}' not found"))?
                    .code
                    .clone();
                self.start_edit_preparation(
                    request_id,
                    EditPreparationInput::Persist {
                        target,
                        source,
                        preview,
                        run_tests,
                    },
                    jit,
                )
            }
        })();
        match result {
            Ok((kind, data)) => LiveResponse::success(request_id, tick, kind, data),
            Err(error) => LiveResponse::failure(request_id, tick, error),
        }
    }

    fn load_files(&self) -> Result<Vec<WorkshopSourceFile>, String> {
        load_workshop_edit_workspace(&self.config.project_root, &self.config.entry)
    }

    fn symbols(
        &self,
        query: Option<&str>,
        kind: Option<&str>,
        files: &[String],
        owner: Option<&str>,
        page: u32,
        limit: usize,
    ) -> Result<(&'static str, Value), String> {
        let query = query.unwrap_or("").to_ascii_lowercase();
        let kind = kind.map(parse_kind).transpose()?;
        if files.len() > 16 {
            return Err("symbol search accepts at most 16 starting files".to_string());
        }
        let loaded_files = &self.source_files;
        let default_scope = files.is_empty();
        let mut scope_files = if default_scope {
            vec![normalize_file(&self.config.entry.to_string_lossy())]
        } else {
            files.iter().map(|file| normalize_file(file)).collect()
        };
        if default_scope {
            scope_files.extend(workshop_direct_import_files(
                loaded_files,
                &self.config.entry,
            )?);
        }
        let available_files = self
            .source_items
            .iter()
            .map(|item| normalize_file(&item.file))
            .collect::<BTreeSet<_>>();
        for file in &scope_files {
            if !available_files.contains(file) {
                return Err(format!("symbol search file is not in the project: {file}"));
            }
        }
        let scope_files = scope_files.into_iter().collect::<BTreeSet<_>>();
        let imports = scope_files
            .iter()
            .map(|file| {
                Ok((
                    file.clone(),
                    workshop_direct_import_files(loaded_files, Path::new(file))?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let matches = |item: &WorkshopSourceItem| {
            item.kind != WorkshopSourceItemKind::Imports
                && !(item.kind == WorkshopSourceItemKind::Globals && item.source.trim().is_empty())
                && (query.is_empty()
                    || item.name.to_ascii_lowercase().contains(&query)
                    || item.signature.to_ascii_lowercase().contains(&query))
                && kind.is_none_or(|kind| item.kind == kind)
                && scope_files.contains(&normalize_file(&item.file))
                && owner.is_none_or(|owner| item.owner.as_deref() == Some(owner))
        };
        let limit = limit.clamp(1, 200);
        let offset = usize::try_from(page)
            .unwrap_or(usize::MAX)
            .saturating_mul(limit);
        let total = self
            .source_items
            .iter()
            .filter(|item| matches(item))
            .count();
        let items = self
            .source_items
            .iter()
            .filter(|item| matches(item))
            .skip(offset)
            .take(limit)
            .map(|item| {
                let mut value = json!({
                    "kind": item.kind,
                    "name": item.name,
                    "file": item.file,
                    "signature": item.signature,
                });
                if let Some(owner) = &item.owner {
                    value["owner"] = Value::String(owner.clone());
                }
                value
            })
            .collect::<Vec<_>>();
        Ok((
            "symbols",
            json!({"schema_version": 1, "files": scope_files, "imports": imports, "page": page, "limit": limit, "total": total, "items": items}),
        ))
    }

    fn read_symbol(&self, target: LiveSymbolTarget) -> Result<(&'static str, Value), String> {
        let selector = selector(&target)?;
        let normalized_file = selector.file.as_deref().map(normalize_file);
        let mut items = self
            .source_items
            .iter()
            .filter(|item| {
                item.name == selector.name
                    && selector.kind.is_none_or(|kind| item.kind == kind)
                    && normalized_file
                        .as_deref()
                        .is_none_or(|file| normalize_file(&item.file) == file)
                    && selector
                        .owner
                        .as_deref()
                        .is_none_or(|owner| item.owner.as_deref() == Some(owner))
                    && selector
                        .signature
                        .as_deref()
                        .is_none_or(|signature| item.signature == signature)
            })
            .cloned()
            .collect::<Vec<_>>();
        if items.len() != 1 {
            return Err(format!(
                "live symbol read requires exactly one code-aware match; found {}",
                items.len()
            ));
        }
        Ok((
            "symbol",
            serde_json::to_value(items.remove(0)).map_err(|error| error.to_string())?,
        ))
    }

    fn start_edit_preparation(
        &mut self,
        request_id: u64,
        input: EditPreparationInput,
        active: &JitProcess,
    ) -> Result<(&'static str, Value), String> {
        if let Some(preparation) = self.edit_preparation.as_ref() {
            return Err(format!(
                "live edit request {} is still preparing; cancel or wait for it",
                preparation.request_id
            ));
        }
        validate_edit_input_size(&input)?;
        let config = self.config.clone();
        let active_layout = active.state_layout();
        let candidate = active.staged_candidate();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_canceled = Arc::clone(&canceled);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name(format!("stasis-live-edit-{request_id}"))
            .spawn(move || {
                let result = prepare_edit(
                    request_id,
                    &config,
                    input,
                    &active_layout,
                    candidate,
                    &worker_canceled,
                );
                let _ = sender.send(result);
            })
            .map_err(|error| format!("failed starting live edit preparation: {error}"))?;
        self.edit_preparation = Some(EditPreparation {
            request_id,
            canceled,
            receiver,
            worker: Some(worker),
        });
        Ok((
            "edit_preparing",
            json!({"request_id": request_id, "background": true}),
        ))
    }

    fn start_completion_preparation(
        &mut self,
        request_id: u64,
        buffer: String,
        cursor: usize,
        limit: usize,
        context: CompletionContext,
    ) -> Result<(&'static str, Value), String> {
        if let Some(preparation) = self.completion_preparation.as_ref() {
            return Err(format!(
                "live completion request {} is still preparing; retry after it finishes",
                preparation.request_id
            ));
        }
        let snapshot = Arc::clone(&self.completion_snapshot);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name(format!("stasis-live-completion-{request_id}"))
            .spawn(move || {
                let query = snapshot.language.query_with_index(
                    snapshot.index.clone(),
                    &buffer,
                    cursor,
                    limit,
                    &context,
                );
                let _ = sender.send(query);
            })
            .map_err(|error| format!("failed starting live completion analysis: {error}"))?;
        self.completion_preparation = Some(CompletionPreparation {
            request_id,
            receiver,
            worker: Some(worker),
        });
        Ok((
            "completion_preparing",
            json!({"request_id": request_id, "background": true}),
        ))
    }

    fn finish_completion_preparation(&mut self, tick: u64) -> Option<LiveResponse> {
        let preparation = self.completion_preparation.as_ref()?;
        let query = match preparation.receiver.try_recv() {
            Ok(query) => query,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                let request_id = preparation.request_id;
                self.completion_preparation = None;
                return Some(LiveResponse::failure(
                    request_id,
                    tick,
                    "live completion analysis worker disconnected",
                ));
            }
        };
        let mut preparation = self
            .completion_preparation
            .take()
            .expect("completion preparation exists");
        if let Some(worker) = preparation.worker.take() {
            let _ = worker.join();
        }
        Some(LiveResponse::success(
            preparation.request_id,
            tick,
            "completion",
            json!(query),
        ))
    }

    fn finish_edit_preparation(
        &mut self,
        tick: u64,
        jit: &mut JitProcess,
        tick_code_ptr: &mut u64,
        render_code_ptr: &mut u64,
    ) -> Option<LiveResponse> {
        let preparation = self.edit_preparation.as_ref()?;
        let result = match preparation.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("live edit preparation worker disconnected".to_string())
            }
        };
        let mut preparation = self.edit_preparation.take().expect("preparation exists");
        if let Some(worker) = preparation.worker.take() {
            let _ = worker.join();
        }
        if preparation.canceled.load(Ordering::Acquire) {
            return Some(LiveResponse::failure(
                preparation.request_id,
                tick,
                "live edit preparation canceled",
            ));
        }
        let mut prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                return Some(LiveResponse::failure(preparation.request_id, tick, error));
            }
        };
        finalize_runtime_preview(&prepared.candidate, &mut prepared.swap_preview);
        if prepared
            .expected_swap_preview
            .as_ref()
            .is_some_and(|expected| expected != &prepared.swap_preview)
        {
            return Some(LiveResponse::failure(
                prepared.request_id,
                tick,
                "refusing live commit because the regenerated swap preview differs from the validated preview",
            ));
        }
        if matches!(prepared.action, PreparedAction::Preview) {
            self.pending_plan = Some(PendingEdit {
                plan: prepared.plan.clone(),
                swap_preview: prepared.swap_preview.clone(),
            });
            return Some(LiveResponse::success(
                prepared.request_id,
                tick,
                "edit_preview",
                json!({
                    "validated": prepared.swap_preview.state_layout_compatible,
                    "plan": prepared.plan,
                    "swap": prepared.swap_preview,
                }),
            ));
        }
        let request_id = prepared.request_id;
        match self.commit_prepared(prepared, jit, tick_code_ptr, render_code_ptr) {
            Ok((kind, data)) => Some(LiveResponse::success(request_id, tick, kind, data)),
            Err(error) => Some(LiveResponse::failure(request_id, tick, error)),
        }
    }

    fn commit_prepared(
        &mut self,
        prepared: PreparedEdit,
        jit: &mut JitProcess,
        tick_code_ptr: &mut u64,
        render_code_ptr: &mut u64,
    ) -> Result<(&'static str, Value), String> {
        verify_prepared_input_hashes(&self.config, &prepared.input_hashes)?;
        self.sync_host_entry_revision();
        let next_host_entry_revision = self.host_entry_revision.saturating_add(1);
        let host_entry_targets = prepared
            .package
            .host_entry_targets(next_host_entry_revision)?;
        stasis_dynload::validate_jit_host_entry_targets(&host_entry_targets)?;
        let active_layout = jit.state_layout();
        let active_version = state_layout_version(&active_layout)?;
        if active_version != prepared.swap_preview.from_layout_version {
            return Err(format!(
                "refusing live commit because active state layout changed after preview (expected {}, found {})",
                prepared.swap_preview.from_layout_version, active_version
            ));
        }
        if !prepared.swap_preview.state_layout_compatible {
            return Err(prepared
                .swap_preview
                .rejection
                .clone()
                .unwrap_or_else(|| "incoming state layout is incompatible".to_string()));
        }
        let receipt_directory = self.config.output.join("live-edits");
        let serialized_plan = serde_json::to_string(&prepared.plan)
            .map_err(|error| format!("failed serializing live receipt: {error}"))?;
        let expected_receipt =
            receipt_directory.join(format!("{}.json", workshop_source_hash(&serialized_plan)));
        let receipt_existed = self.config.project_root.join(&expected_receipt).is_file();
        self.remember_plan_hashes(&prepared.plan, prepared.restore);
        if let Err(error) = write_workshop_semantic_plan(
            &self.config.project_root,
            &prepared.plan,
            prepared.restore,
        ) {
            self.remember_plan_hashes(&prepared.plan, !prepared.restore);
            return Err(error);
        }
        let receipt = match write_workshop_semantic_receipt(
            &self.config.project_root,
            &receipt_directory,
            &prepared.plan,
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                self.rollback_prepared(&prepared)?;
                return Err(format!(
                    "live receipt failed; disk/runtime remained unchanged: {error}"
                ));
            }
        };

        let hook = prepared.package.on_code_swap_code_ptr;
        let runtime_result = activate_candidate_transactionally(
            Some(jit),
            &prepared.candidate,
            &prepared.swap_preview,
            hook.is_some(),
            || {
                hook.map_or(Ok(()), |code_ptr| {
                    stasis_dynload::invoke_code_swap_hook(code_ptr as usize)
                })
            },
            Result::is_ok,
        );
        let runtime_result = match runtime_result {
            Ok(result) => result,
            Err(error) => {
                self.rollback_prepared(&prepared)?;
                cleanup_new_receipt(&self.config, &receipt, receipt_existed)?;
                return Err(format!(
                    "live runtime transaction failed; disk/code/state remained on the prior version: {error}"
                ));
            }
        };
        if let Err(error) = runtime_result {
            self.rollback_prepared(&prepared)?;
            cleanup_new_receipt(&self.config, &receipt, receipt_existed)?;
            return Err(format!(
                "on_code_swap failed; disk/code/state remained on the prior version: {error}"
            ));
        }
        stasis_dynload::publish_jit_host_entry_targets(host_entry_targets)?;
        self.host_entry_revision = next_host_entry_revision;
        *tick_code_ptr = stasis_dynload::jit_host_tick_trampoline_ptr() as u64;
        *render_code_ptr = stasis_dynload::jit_host_render_trampoline_ptr() as u64;
        let plan = prepared.plan.clone();
        let swap_preview = prepared.swap_preview.clone();
        let action = prepared.action.clone();
        let source_items = prepared.source_items;
        let completion_items = prepared.completion_items;
        let source_files = prepared.source_files;
        let tests = if prepared.tests_ran {
            "passed"
        } else {
            "skipped"
        };
        let patch_status = prepared.candidate.generation_metadata().map_or_else(
            || json!({"revision": next_host_entry_revision}),
            |metadata| {
                json!({
                    "revision": next_host_entry_revision,
                    "source_revision": metadata.source_revision,
                    "re_jit_count": metadata.emitted_function_ids.len(),
                    "reused_count": metadata.reused_function_ids.len(),
                    "codegen_micros": metadata.codegen_micros,
                    "plan_micros": metadata.plan_micros,
                    "finalize_micros": metadata.finalize_micros,
                    "retained_arena_count": metadata.retained_arena_count,
                    "retained_jit_bytes": metadata.retained_jit_bytes,
                    "total_jit_bytes": metadata.total_jit_bytes,
                })
            },
        );
        *jit = prepared.candidate;
        self.source_items = source_items;
        self.completion_items = completion_items;
        self.source_files = source_files;
        self.remember_plan_hashes(&plan, prepared.restore);
        let (kind, data) = match action {
            PreparedAction::ApplyNew => {
                self.history.truncate(self.history_cursor);
                self.history.push(HistoryEntry {
                    plan: plan.clone(),
                    swap_preview: swap_preview.clone(),
                    receipt: receipt.clone(),
                });
                self.history_cursor = self.history.len();
                (
                    "edit_applied",
                    json!({"plan": plan, "swap": swap_preview, "receipt": receipt, "tests": tests, "jit_patch": patch_status}),
                )
            }
            PreparedAction::ApplyPending => {
                self.history.truncate(self.history_cursor);
                self.history.push(HistoryEntry {
                    plan: plan.clone(),
                    swap_preview: swap_preview.clone(),
                    receipt: receipt.clone(),
                });
                self.history_cursor = self.history.len();
                self.pending_plan = None;
                (
                    "edit_applied",
                    json!({"plan": plan, "swap": swap_preview, "receipt": receipt, "tests": tests, "jit_patch": patch_status}),
                )
            }
            PreparedAction::Undo { index } => {
                self.history_cursor = index;
                (
                    "edit_undone",
                    json!({"index": index, "swap": swap_preview, "receipt": receipt, "tests": tests, "jit_patch": patch_status}),
                )
            }
            PreparedAction::Redo { index } => {
                self.history_cursor = index + 1;
                (
                    "edit_redone",
                    json!({"index": index, "swap": swap_preview, "receipt": receipt, "tests": tests, "jit_patch": patch_status}),
                )
            }
            PreparedAction::Preview => unreachable!("preview never commits"),
        };
        self.rebuild_completion(jit);
        Ok((kind, data))
    }

    fn rollback_prepared(&mut self, prepared: &PreparedEdit) -> Result<(), String> {
        let result = rollback_prepared_disk(&self.config, prepared);
        self.remember_plan_hashes(&prepared.plan, !prepared.restore);
        result
    }

    fn remember_plan_hashes(&mut self, plan: &WorkshopSemanticEditPlan, restore: bool) {
        for change in &plan.changed_files {
            let source = if restore {
                &change.before_source
            } else {
                &change.after_source
            };
            let path = self.config.project_root.join(&change.file);
            let path = path.canonicalize().unwrap_or(path);
            self.self_write_hashes
                .insert(path, workshop_source_hash(source));
        }
    }

    fn refresh_completion(&mut self, jit: &JitProcess) -> Result<(), String> {
        let files = self.load_files()?;
        self.source_items = workshop_source_items(&files)?;
        self.completion_items = workshop_completion_items(&files)?;
        self.source_files = files;
        self.rebuild_completion(jit);
        Ok(())
    }

    fn rebuild_completion(&mut self, jit: &JitProcess) {
        let mut items = live_command_completions();
        items.extend(
            self.completion_items
                .iter()
                .filter(|item| !is_static_type_field(item))
                .map(live_completion_item),
        );
        items.extend(
            jit.global_scalar_paths()
                .into_iter()
                .map(|(path, type_name)| CompletionItem {
                    text: path,
                    kind: "state_path".to_string(),
                    detail: type_name.to_string(),
                    type_name: Some(type_name.to_string()),
                    source: Some("runtime state".to_string()),
                    selector: None,
                    scope: None,
                }),
        );
        items.extend(self.scratch.list().into_iter().map(|cell| CompletionItem {
            text: cell.name,
            kind: "scratch_cell".to_string(),
            detail: "session-only scratch cell".to_string(),
            type_name: None,
            source: Some("scratch workspace".to_string()),
            selector: None,
            scope: None,
        }));
        self.completion.replace(items);
        self.completion_snapshot = Arc::new(CompletionSnapshot {
            index: self.completion.clone(),
            language: LanguageCompletionSnapshot::new(
                self.source_items.clone(),
                self.source_files.clone(),
            ),
        });
    }

    #[cfg(test)]
    fn completion_query(
        &self,
        buffer: &str,
        cursor: usize,
        limit: usize,
        context: &stasis_runner::live::CompletionContext,
    ) -> stasis_runner::live::CompletionQuery {
        self.completion_snapshot.language.query_with_index(
            self.completion.clone(),
            buffer,
            cursor,
            limit,
            context,
        )
    }
}

fn is_static_type_field(item: &WorkshopCompletionItem) -> bool {
    item.kind == "field"
        && item.scope.is_none()
        && item.owner.as_deref().is_some_and(|owner| {
            item.text
                .strip_prefix(owner)
                .is_some_and(|suffix| suffix.starts_with('.'))
        })
}

fn live_completion_item(item: &WorkshopCompletionItem) -> CompletionItem {
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
        scope: item.scope.as_ref().map(|scope| CompletionScope {
            owner: scope.owner.clone(),
            file: scope.file.clone(),
            owner_signature: scope.owner_signature.clone(),
            owner_end: scope.owner_end,
            visible_from: scope.visible_from,
            visible_to: scope.visible_to,
        }),
    }
}

fn paged_completion_query(
    index: &CompletionIndex,
    query: &str,
    page: u32,
    limit: usize,
    context: &stasis_runner::live::CompletionContext,
) -> stasis_runner::live::CompletionQuery {
    let start = (page as usize).saturating_mul(limit);
    let requested = start.saturating_add(limit).min(256);
    let mut result = index.query_with_context(query, query.len(), requested, context);
    result.page = page;
    result.items = result.items.into_iter().skip(start).take(limit).collect();
    result
}

fn validate_edit_input_size(input: &EditPreparationInput) -> Result<(), String> {
    let source = match input {
        EditPreparationInput::Edit { source, .. } => source.as_deref(),
        EditPreparationInput::EditBatch { edits, .. } => {
            if edits.len() > MAX_LIVE_EDIT_BATCH {
                return Err(format!(
                    "live edit batch exceeds {MAX_LIVE_EDIT_BATCH} edits"
                ));
            }
            let bytes = edits
                .iter()
                .filter_map(|edit| edit.source.as_deref())
                .map(str::len)
                .sum::<usize>();
            if bytes > MAX_LIVE_EDIT_SOURCE_BYTES {
                return Err(format!(
                    "live edit source exceeds {MAX_LIVE_EDIT_SOURCE_BYTES} bytes"
                ));
            }
            None
        }
        EditPreparationInput::Persist { source, .. } => Some(source.as_str()),
        EditPreparationInput::Plan { .. } => None,
    };
    if source.is_some_and(|source| source.len() > MAX_LIVE_EDIT_SOURCE_BYTES) {
        return Err(format!(
            "live edit source exceeds {MAX_LIVE_EDIT_SOURCE_BYTES} bytes"
        ));
    }
    Ok(())
}

fn prepare_edit(
    request_id: u64,
    config: &LiveRunConfig,
    input: EditPreparationInput,
    active_layout: &JitStateLayout,
    candidate: JitProcess,
    canceled: &AtomicBool,
) -> Result<PreparedEdit, String> {
    check_preparation_canceled(canceled)?;
    let files = load_workshop_edit_workspace(&config.project_root, &config.entry)?;
    let input_hashes = files
        .iter()
        .map(|file| (file.path.clone(), workshop_source_hash(&file.source)))
        .collect::<BTreeMap<_, _>>();
    let (candidate_files, plan, restore, run_tests, mut action, expected_swap_preview) = match input
    {
        EditPreparationInput::Edit {
            operation,
            target,
            source,
            expected_source_hash,
            preview,
            run_tests,
        } => {
            let (after, plan) =
                plan_live_edit(&files, operation, target, source, expected_source_hash)?;
            (
                after,
                plan,
                false,
                run_tests && !preview,
                if preview {
                    PreparedAction::Preview
                } else {
                    PreparedAction::ApplyNew
                },
                None,
            )
        }
        EditPreparationInput::EditBatch {
            edits,
            preview,
            run_tests,
        } => {
            let (after, plan) = plan_live_edit_batch(&files, edits)?;
            (
                after,
                plan,
                false,
                run_tests && !preview,
                if preview {
                    PreparedAction::Preview
                } else {
                    PreparedAction::ApplyNew
                },
                None,
            )
        }
        EditPreparationInput::Persist {
            target,
            source,
            preview,
            run_tests,
        } => {
            let operation = if find_workshop_symbols(&files, &selector(&target)?)?.is_empty() {
                LiveEditOperation::Add
            } else {
                LiveEditOperation::Update
            };
            let (after, plan) = plan_live_edit(&files, operation, target, Some(source), None)?;
            (
                after,
                plan,
                false,
                run_tests && !preview,
                if preview {
                    PreparedAction::Preview
                } else {
                    PreparedAction::ApplyNew
                },
                None,
            )
        }
        EditPreparationInput::Plan {
            plan,
            expected_swap_preview,
            restore,
            run_tests,
            action,
        } => {
            let candidate_files = files_for_plan(&files, &plan, restore)?;
            (
                candidate_files,
                plan,
                restore,
                run_tests,
                action,
                expected_swap_preview,
            )
        }
    };
    require_semantic_changes(&plan)?;
    check_preparation_canceled(canceled)?;
    let (candidate, package) = compile_candidate(config, &candidate_files, candidate)?;
    let snapshot = candidate
        .program_snapshot()
        .ok_or_else(|| "live edit candidate has no ProgramSnapshot".to_string())?;
    let metadata = candidate
        .generation_metadata()
        .ok_or_else(|| "live edit candidate has no generation metadata".to_string())?;
    let changed_functions = metadata
        .emitted_function_ids
        .iter()
        .map(|function_id| {
            snapshot
                .function_by_id(*function_id)
                .map(|function| function.symbol_id.to_string())
                .ok_or_else(|| format!("emitted FnId {function_id:08x} is missing from snapshot"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let abi_rejection = (plan.reload.expected_reload == ExpectedReload::ResetRequired
        && plan
            .reload
            .reason
            .to_ascii_lowercase()
            .contains("signature changed"))
    .then(|| {
        format!(
            "{} Hot reload cannot migrate function ABI changes.",
            plan.reload.reason
        )
    });
    let swap_preview = plan_state_migration(
        active_layout,
        &candidate.state_layout(),
        changed_functions,
        plan.reload.expected_reload == ExpectedReload::ResetRequired,
        abi_rejection,
    )?;
    if matches!(action, PreparedAction::ApplyNew) && swap_preview.requires_explicit_apply {
        action = PreparedAction::Preview;
    }
    if !matches!(action, PreparedAction::Preview) && !swap_preview.state_layout_compatible {
        return Err(swap_preview
            .rejection
            .clone()
            .unwrap_or_else(|| "incoming state layout is incompatible".to_string()));
    }
    check_preparation_canceled(canceled)?;
    if run_tests {
        run_staged_tests(config, &candidate_files, request_id, canceled).map_err(|error| {
            format!("live edit tests failed; disk/runtime remained unchanged: {error}")
        })?;
    }
    check_preparation_canceled(canceled)?;
    let source_items = workshop_source_items(&candidate_files)?;
    let completion_items = workshop_completion_items(&candidate_files)?;
    Ok(PreparedEdit {
        request_id,
        plan,
        swap_preview,
        expected_swap_preview,
        restore,
        action,
        tests_ran: run_tests,
        candidate,
        package,
        source_items,
        completion_items,
        source_files: candidate_files,
        input_hashes,
    })
}

fn require_semantic_changes(plan: &WorkshopSemanticEditPlan) -> Result<(), String> {
    if plan.reload.changed_symbols.is_empty() {
        Err("live edit has no semantic changes; disk and runtime were left unchanged".to_string())
    } else {
        Ok(())
    }
}

fn plan_live_edit(
    files: &[WorkshopSourceFile],
    operation: LiveEditOperation,
    target: LiveSymbolTarget,
    source: Option<String>,
    expected_source_hash: Option<String>,
) -> Result<(Vec<WorkshopSourceFile>, WorkshopSemanticEditPlan), String> {
    let mut target = selector(&target)?;
    let operation = match operation {
        LiveEditOperation::Add => WorkshopSemanticEditOperation::Add,
        LiveEditOperation::Update => WorkshopSemanticEditOperation::Update,
        LiveEditOperation::Delete => WorkshopSemanticEditOperation::Delete,
    };
    if operation == WorkshopSemanticEditOperation::Add && target.file.is_none() {
        return Err("code-aware live add requires a project src/ or tests/ file".to_string());
    }
    if operation != WorkshopSemanticEditOperation::Add && target.file.is_none() {
        let matches = find_workshop_symbols(files, &target)?;
        if matches.len() != 1 {
            return Err(format!(
                "code-aware live edit requires one target; found {}",
                matches.len()
            ));
        }
        target.file = Some(matches[0].file.clone());
    }
    plan_workshop_semantic_edits(
        files,
        &WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation,
                target,
                new_source: source,
                expected_source_hash,
            }],
        },
    )
}

fn plan_live_edit_batch(
    files: &[WorkshopSourceFile],
    edits: Vec<stasis_runner::live::LiveEdit>,
) -> Result<(Vec<WorkshopSourceFile>, WorkshopSemanticEditPlan), String> {
    if edits.is_empty() {
        return Err("live edit batch must contain at least one edit".to_string());
    }
    let mut semantic_edits = Vec::with_capacity(edits.len());
    for edit in edits {
        let mut target = selector(&edit.target)?;
        let operation = match edit.operation {
            LiveEditOperation::Add => WorkshopSemanticEditOperation::Add,
            LiveEditOperation::Update => WorkshopSemanticEditOperation::Update,
            LiveEditOperation::Delete => WorkshopSemanticEditOperation::Delete,
        };
        if operation == WorkshopSemanticEditOperation::Add && target.file.is_none() {
            return Err("code-aware live add requires a project src/ or tests/ file".to_string());
        }
        if operation != WorkshopSemanticEditOperation::Add && target.file.is_none() {
            let matches = find_workshop_symbols(files, &target)?;
            if matches.len() != 1 {
                return Err(format!(
                    "code-aware live edit requires one target; found {}",
                    matches.len()
                ));
            }
            target.file = Some(matches[0].file.clone());
        }
        semantic_edits.push(WorkshopSemanticEdit {
            operation,
            target,
            new_source: edit.source,
            expected_source_hash: edit.expected_source_hash,
        });
    }
    plan_workshop_semantic_edits(
        files,
        &WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: semantic_edits,
        },
    )
}

fn compile_candidate(
    config: &LiveRunConfig,
    files: &[WorkshopSourceFile],
    mut candidate: JitProcess,
) -> Result<(JitProcess, JitEnginePackage), String> {
    let runtime_files = workshop_reachable_files(files, &config.entry)?;
    candidate.set_project_root(config.project_root.to_string_lossy())?;
    candidate.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
        "on_code_swap".to_string(),
    ]);
    let mut retained_paths = BTreeSet::new();
    for file in runtime_files {
        let path = config.project_root.join(&file.path);
        let path = path.canonicalize().unwrap_or(path);
        let path = path.to_string_lossy().to_string();
        retained_paths.insert(path.clone());
        candidate.upsert_file(path, file.source);
    }
    candidate.retain_files(&retained_paths);
    candidate
        .compile_staged()
        .map_err(|error| candidate_diagnostic(&candidate, error))?;
    let package = candidate
        .build_engine_package(&EngineEntrypoints::runtime_default())
        .map_err(|error| format!("live engine package failed: {error}"))?;
    Ok((candidate, package))
}

fn run_staged_tests(
    config: &LiveRunConfig,
    files: &[WorkshopSourceFile],
    request_id: u64,
    canceled: &AtomicBool,
) -> Result<(), String> {
    let stamp = workshop_source_hash(
        &files
            .iter()
            .map(|file| format!("{}:{}", file.path, workshop_source_hash(&file.source)))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let root = std::env::temp_dir().join(format!(
        "stasis-live-prepare-{}-{request_id}-{}",
        std::process::id(),
        &stamp[..stamp.len().min(16)]
    ));
    if root.exists() {
        fs::remove_dir_all(&root)
            .map_err(|error| format!("failed clearing live test overlay: {error}"))?;
    }
    fs::create_dir_all(&root)
        .map_err(|error| format!("failed creating live test overlay: {error}"))?;
    let result = (|| {
        let manifest = config.project_root.join("stasis.json");
        if manifest.is_file() {
            fs::copy(&manifest, root.join("stasis.json"))
                .map_err(|error| format!("failed staging stasis.json: {error}"))?;
        }
        for file in files {
            check_preparation_canceled(canceled)?;
            let destination = root.join(&file.path);
            if !destination.starts_with(&root) {
                return Err(format!("refusing live test overlay path {}", file.path));
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed creating live test directory: {error}"))?;
            }
            fs::write(&destination, &file.source)
                .map_err(|error| format!("failed staging {}: {error}", file.path))?;
        }
        let executable = locate_stasis_executable()?.ok_or_else(|| {
            "isolated staged tests require a stasis executable beside the running test binary"
                .to_string()
        })?;
        run_staged_test_process(&executable, &root, canceled)
    })();
    let cleanup = fs::remove_dir_all(&root);
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(format!("failed cleaning live test overlay: {error}")),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn locate_stasis_executable() -> Result<Option<PathBuf>, String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("failed locating stasis test executable: {error}"))?;
    if current.file_stem().and_then(|stem| stem.to_str()) == Some("stasis") {
        return Ok(Some(current));
    }
    let Some(debug_directory) = current.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let candidate = debug_directory.join(if cfg!(windows) {
        "stasis.exe"
    } else {
        "stasis"
    });
    Ok(candidate.is_file().then_some(candidate))
}

fn run_staged_test_process(
    executable: &Path,
    root: &Path,
    canceled: &AtomicBool,
) -> Result<(), String> {
    let mut child = Command::new(executable)
        .args(["--json", "test"])
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed starting staged live tests: {error}"))?;
    let total_bytes = Arc::new(AtomicUsize::new(0));
    let output_overflow = Arc::new(AtomicBool::new(false));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "staged test stdout pipe is unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "staged test stderr pipe is unavailable".to_string())?;
    let stdout_total = Arc::clone(&total_bytes);
    let stdout_overflow = Arc::clone(&output_overflow);
    let stdout_worker = std::thread::spawn(move || {
        drain_bounded_test_output(stdout, &stdout_total, &stdout_overflow)
    });
    let stderr_total = Arc::clone(&total_bytes);
    let stderr_overflow = Arc::clone(&output_overflow);
    let stderr_worker = std::thread::spawn(move || {
        drain_bounded_test_output(stderr, &stderr_total, &stderr_overflow)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let outcome = loop {
        if canceled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            break Err("live edit preparation canceled during tests".to_string());
        }
        if output_overflow.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            break Err(format!(
                "staged live test output exceeded {MAX_STAGED_TEST_OUTPUT_BYTES} bytes"
            ));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break Err("staged live tests exceeded 300 seconds".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("failed polling staged live tests: {error}"));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let stdout = stdout_worker
        .join()
        .map_err(|_| "staged test stdout reader panicked".to_string())??;
    let stderr = stderr_worker
        .join()
        .map_err(|_| "staged test stderr reader panicked".to_string())??;
    if output_overflow.load(Ordering::Acquire) {
        return Err(format!(
            "staged live test output exceeded {MAX_STAGED_TEST_OUTPUT_BYTES} bytes"
        ));
    }
    let status = outcome?;
    if status.success() {
        return Ok(());
    }
    let diagnostics = format!("{stdout}{stderr}");
    Err(format!(
        "staged live tests failed: {}",
        diagnostics
            .chars()
            .take(MAX_STAGED_TEST_DIAGNOSTIC_BYTES)
            .collect::<String>()
    ))
}

fn drain_bounded_test_output(
    mut reader: impl Read,
    total_bytes: &AtomicUsize,
    overflow: &AtomicBool,
) -> Result<String, String> {
    let mut captured = Vec::with_capacity(MAX_STAGED_TEST_DIAGNOSTIC_BYTES);
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("failed draining staged test output: {error}"))?;
        if count == 0 {
            break;
        }
        let previous = total_bytes.fetch_add(count, Ordering::AcqRel);
        if previous.saturating_add(count) > MAX_STAGED_TEST_OUTPUT_BYTES {
            overflow.store(true, Ordering::Release);
        }
        let remaining = MAX_STAGED_TEST_DIAGNOSTIC_BYTES.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    Ok(String::from_utf8_lossy(&captured).into_owned())
}

fn check_preparation_canceled(canceled: &AtomicBool) -> Result<(), String> {
    if canceled.load(Ordering::Acquire) {
        Err("live edit preparation canceled".to_string())
    } else {
        Ok(())
    }
}

fn rollback_prepared_disk(config: &LiveRunConfig, prepared: &PreparedEdit) -> Result<(), String> {
    write_workshop_semantic_plan(&config.project_root, &prepared.plan, !prepared.restore)
        .map_err(|error| format!("disk rollback failed: {error}"))
}

fn verify_prepared_input_hashes(
    config: &LiveRunConfig,
    expected: &BTreeMap<String, String>,
) -> Result<(), String> {
    let current = load_workshop_edit_workspace(&config.project_root, &config.entry)?
        .into_iter()
        .map(|file| (file.path, workshop_source_hash(&file.source)))
        .collect::<BTreeMap<_, _>>();
    if current == *expected {
        return Ok(());
    }
    let changed = current
        .keys()
        .chain(expected.keys())
        .find(|path| current.get(*path) != expected.get(*path))
        .cloned()
        .unwrap_or_else(|| "project sources".to_string());
    Err(format!(
        "refusing live commit because {changed} changed during background preparation"
    ))
}

fn cleanup_new_receipt(
    config: &LiveRunConfig,
    receipt: &Path,
    receipt_existed: bool,
) -> Result<(), String> {
    if receipt_existed {
        return Ok(());
    }
    fs::remove_file(config.project_root.join(receipt))
        .map_err(|error| format!("receipt cleanup failed: {error}"))
}

fn help_data() -> Value {
    json!({
        "commands": [
            ":help", ":status", ":pause", ":resume", ":step [ticks]", ":cancel REQUEST_ID", ":quit",
            ":symbols [query] [--file PATH ... --kind KIND --owner OWNER --page N --limit N]",
            ":read NAME [KIND] [--file FILE --owner OWNER --signature SIGNATURE]",
            ":references SYMBOL [--limit N]", ":validate PATH OP VALUE [--frames N]",
            ":edit SYMBOL (interactive TUI)",
            ":ai PROMPT | :ai status | :ai cancel (interactive TUI; installed Codex subscription)",
            ":complete BUFFER", ":palette [QUERY]",
            ":add KIND NAME FILE ... :end", ":update KIND NAME [FILE] ... :end",
            ":delete KIND NAME [FILE]", ":preview", ":apply", ":changes", ":undo", ":redo",
            ":inspect [QUERY]", ":watch QUERY", ":unwatch [QUERY]",
            ":set PATH VALUE",
            ":print SCALAR_EXPRESSION", ":do ... :end", ":cell put|run|list|clear"
        ],
        "multiline_terminator": ":end",
        "multiline_cancel": ":abort or Ctrl-C",
        "line_editor": "session history; Ctrl-P opens the compiler-backed palette; Tab completes",
        "durability": "semantic edits persist through code-aware receipts; scratch cells do not persist unless explicitly promoted",
    })
}

fn live_command_completions() -> Vec<CompletionItem> {
    [
        ":help",
        ":status",
        ":pause",
        ":resume",
        ":step",
        ":cancel",
        ":quit",
        ":symbols",
        ":find",
        ":read",
        ":references",
        ":validate",
        ":edit",
        ":ai",
        ":complete",
        ":palette",
        ":add",
        ":update",
        ":delete",
        ":preview",
        ":apply",
        ":changes",
        ":undo",
        ":redo",
        ":inspect",
        ":watch",
        ":unwatch",
        ":set",
        ":print",
        ":do",
        ":cell",
    ]
    .into_iter()
    .map(|text| CompletionItem {
        text: text.to_string(),
        kind: "command".to_string(),
        detail: "live command".to_string(),
        type_name: None,
        source: Some("interactive workspace".to_string()),
        selector: None,
        scope: None,
    })
    .collect()
}

fn selector(target: &LiveSymbolTarget) -> Result<WorkshopSymbolSelector, String> {
    Ok(WorkshopSymbolSelector {
        symbol_id: None,
        name: target.name.clone(),
        kind: target.kind.as_deref().map(parse_kind).transpose()?,
        file: target.file.as_deref().map(normalize_file),
        owner: target.owner.clone(),
        signature: target.signature.clone(),
    })
}

fn parse_kind(kind: &str) -> Result<WorkshopSourceItemKind, String> {
    match kind.to_ascii_lowercase().as_str() {
        "imports" | "import" => Ok(WorkshopSourceItemKind::Imports),
        "globals" | "global" | "constant" => Ok(WorkshopSourceItemKind::Globals),
        "struct" => Ok(WorkshopSourceItemKind::Struct),
        "function" | "fn" => Ok(WorkshopSourceItemKind::Function),
        "test" => Ok(WorkshopSourceItemKind::Test),
        _ => Err(format!(
            "unsupported symbol kind '{kind}'; use imports, globals, struct, function, or test"
        )),
    }
}

fn normalize_file(file: &str) -> String {
    file.replace('\\', "/").trim_start_matches("./").to_string()
}

fn files_for_plan(
    current: &[WorkshopSourceFile],
    plan: &WorkshopSemanticEditPlan,
    restore: bool,
) -> Result<Vec<WorkshopSourceFile>, String> {
    let mut files = current.to_vec();
    for change in &plan.changed_files {
        let file = files
            .iter_mut()
            .find(|file| file.path == change.file)
            .ok_or_else(|| format!("semantic plan file is no longer imported: {}", change.file))?;
        let expected = if restore {
            &change.after_hash
        } else {
            &change.before_hash
        };
        if workshop_source_hash(&file.source) != *expected {
            return Err(format!(
                "refusing live {} because {} changed since the plan",
                if restore { "undo" } else { "apply" },
                change.file
            ));
        }
        file.source = if restore {
            change.before_source.clone()
        } else {
            change.after_source.clone()
        };
    }
    Ok(files)
}

fn candidate_diagnostic(candidate: &JitProcess, error: CompileError) -> String {
    candidate
        .last_source_diagnostic()
        .map(|diagnostic| {
            format!(
                "{}:{}-{}: {}",
                diagnostic.path, diagnostic.start, diagnostic.end, diagnostic.message
            )
        })
        .unwrap_or_else(|| format!("live compile failed: {error:?}"))
}

fn inspect_scalar(jit: &JitProcess, path: &str) -> Result<(&'static str, Value), String> {
    Ok(("inspection", jit.inspect_state_query(path)?))
}

fn inspection_observed_value(inspection: &Value) -> Value {
    inspection
        .get("value")
        .cloned()
        .unwrap_or_else(|| inspection.clone())
}

fn validate_live_runtime(
    jit: &JitProcess,
    requirement: &stasis_runner::live::LiveValidationRequirement,
    frames: u32,
) -> Result<(&'static str, Value), String> {
    if frames > 600 {
        return Err("frames exceeds the 600-frame limit".to_string());
    }
    let snapshot = stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES)?;
    let result = (|| {
        for _ in 0..frames {
            execute_validation_entry(jit, "tick")?;
        }
        execute_validation_entry(jit, "render")?;
        let actual = serde_json::to_value(jit.read_global_scalar(&requirement.path)?)
            .map_err(|error| format!("failed encoding {}: {error}", requirement.path))?
            .get("value")
            .cloned()
            .ok_or_else(|| {
                format!(
                    "inspection for {} returned no scalar value",
                    requirement.path
                )
            })?;
        let passed = compare_live_validation_values(&actual, &requirement.op, &requirement.value)?;
        Ok((
            "runtime_validation",
            json!({
                "baseline": "live",
                "frames": frames,
                "requirements_met": passed,
                "checks": [{
                    "path": requirement.path,
                    "op": requirement.op,
                    "expected": requirement.value,
                    "actual": actual,
                    "passed": passed,
                }],
            }),
        ))
    })();
    stasis_dynload::restore_jit_runtime_state(&snapshot);
    result
}

fn execute_validation_entry(jit: &JitProcess, name: &str) -> Result<(), String> {
    match jit.execute_i32_noarg_by_name(name) {
        Ok(_) => Ok(()),
        Err(error) if error.contains("not i32-returning") => jit.execute_void_noarg_by_name(name),
        Err(error) => Err(error),
    }
}

fn inspect_all_scalars(
    jit: &JitProcess,
    limit: usize,
    concise: bool,
) -> Result<(&'static str, Value), String> {
    let paths = jit.global_scalar_paths();
    let path_names = paths
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<HashSet<_>>();
    let paths = paths
        .into_iter()
        .filter(|(path, _)| !concise || concise_state_path_is_visible(path, &path_names))
        .collect::<Vec<_>>();
    let total = paths.len();
    let limit = limit.clamp(1, 64);
    let items = paths
        .into_iter()
        .take(limit)
        .map(|(path, static_type)| {
            let value = jit.read_global_scalar(&path)?;
            Ok(json!({"path": path, "static_type": static_type, "value": value}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let memory = jit.state_memory_report(&BTreeMap::new(), MAX_STATE_SNAPSHOT_BYTES as u64)?;
    let layout = jit.state_layout();
    let collections = layout
        .collections
        .iter()
        .take(limit)
        .map(|collection| {
            let active_count = memory
                .entries
                .iter()
                .find(|entry| entry.path == collection.path && entry.kind == "collection_field")
                .and_then(|entry| entry.active_count);
            json!({
                "path": collection.path,
                "kind": "collection",
                "element_shape": collection.element_shape,
                "capacity": collection.capacity,
                "active_count": active_count,
                "fields": collection.fields,
            })
        })
        .collect::<Vec<_>>();
    let structs = layout.structs.iter().take(limit).collect::<Vec<_>>();
    Ok((
        "state_inspection",
        json!({
            "total": total,
            "limit": limit,
            "truncated": total > limit || layout.collections.len() > limit || layout.structs.len() > limit,
            "concise": concise,
            "items": items,
            "collections": collections,
            "structs": structs,
            "memory": {
                "storage_model": memory.storage_model,
                "total_capacity_bytes": memory.total_capacity_bytes,
                "snapshot_bytes": memory.snapshot_bytes,
                "mobile_budget_bytes": memory.mobile_budget_bytes,
                "warnings": memory.warnings,
            },
        }),
    ))
}

fn concise_state_path_is_visible(path: &str, all_paths: &HashSet<String>) -> bool {
    let leaf = path.rsplit('.').next().unwrap_or(path);
    if leaf == "max_length" {
        return false;
    }
    if leaf == "length" {
        let parent = path.strip_suffix(".length").unwrap_or_default();
        if all_paths.contains(format!("{parent}.max_length").as_str()) {
            return false;
        }
    }
    let is_axis = matches!(leaf, "x" | "y" | "z")
        || ["_x", "_y", "_z"]
            .iter()
            .any(|suffix| leaf.ends_with(suffix));
    let is_position = path
        .split('.')
        .any(|segment| matches!(segment, "position" | "positions"));
    !is_axis && !is_position
}

fn print_scalar(jit: &JitProcess, expression: &str) -> Result<(&'static str, Value), String> {
    Ok(("print", jit.inspect_state_query(expression)?))
}

fn evaluate_expression(
    jit: &JitProcess,
    expression: &str,
) -> Result<(&'static str, Value), String> {
    Ok(("evaluation", jit.inspect_state_query(expression)?))
}

fn set_scalar(
    jit: &JitProcess,
    path: &str,
    expression: &str,
    preview: bool,
) -> Result<(&'static str, Value), String> {
    let old = jit.read_global_scalar(path)?;
    let new = parse_scalar_value(jit, expression, old)?;
    if !preview {
        jit.write_global_scalar(path, new)?;
    }
    Ok((
        if preview {
            "mutation_preview"
        } else {
            "mutation_committed"
        },
        json!({"path": path, "static_type": old.type_name(), "old": old, "new": new, "preview": preview}),
    ))
}

fn parse_scalar_value(
    jit: &JitProcess,
    expression: &str,
    expected: JitScalarValue,
) -> Result<JitScalarValue, String> {
    if jit.has_global_path(expression) {
        let value = jit.read_global_scalar(expression)?;
        if value.type_name() == expected.type_name() {
            return Ok(value);
        }
        return Err(format!(
            "state path '{expression}' has type {}, expected {}",
            value.type_name(),
            expected.type_name()
        ));
    }
    match expected {
        JitScalarValue::I32(_) => expression
            .parse::<i32>()
            .map(JitScalarValue::I32)
            .map_err(|error| format!("invalid i32 expression '{expression}': {error}")),
        JitScalarValue::F32(_) => expression
            .parse::<f32>()
            .map(JitScalarValue::F32)
            .map_err(|error| format!("invalid f32 expression '{expression}': {error}")),
        JitScalarValue::F64(_) => expression
            .parse::<f64>()
            .map(JitScalarValue::F64)
            .map_err(|error| format!("invalid f64 expression '{expression}': {error}")),
        JitScalarValue::Bool(_) => match expression {
            "true" => Ok(JitScalarValue::Bool(true)),
            "false" => Ok(JitScalarValue::Bool(false)),
            _ => Err(format!("invalid bool expression '{expression}'")),
        },
        JitScalarValue::U8(_) => expression
            .parse::<u8>()
            .map(JitScalarValue::U8)
            .map_err(|error| format!("invalid u8 expression '{expression}': {error}")),
        JitScalarValue::U16(_) => expression
            .parse::<u16>()
            .map(JitScalarValue::U16)
            .map_err(|error| format!("invalid u16 expression '{expression}': {error}")),
        JitScalarValue::U32(_) => expression
            .parse::<u32>()
            .map(JitScalarValue::U32)
            .map_err(|error| format!("invalid u32 expression '{expression}': {error}")),
    }
}

fn apply_scalar_transaction(
    jit: &JitProcess,
    code: &str,
    preview: bool,
) -> Result<(&'static str, Value), String> {
    if code.len() > MAX_LIVE_EDIT_SOURCE_BYTES {
        return Err(format!(
            "typed live transaction exceeds {MAX_LIVE_EDIT_SOURCE_BYTES} bytes"
        ));
    }
    let mut mutations = Vec::new();
    for raw in code.split(';') {
        let statement = raw.trim();
        if statement.is_empty() {
            continue;
        }
        let (path, expression) = statement.split_once('=').ok_or_else(|| {
            format!("typed live transaction requires PATH = VALUE: '{statement}'")
        })?;
        let path = path.trim();
        let expression = expression.trim();
        if path.contains('(') || expression.contains('(') {
            return Err("calls are not allowed in typed live transactions".to_string());
        }
        let old = jit.read_global_scalar(path)?;
        let new = parse_scalar_value(jit, expression, old)?;
        mutations.push((path.to_string(), old, new));
        if mutations.len() > MAX_LIVE_TRANSACTION_ASSIGNMENTS {
            return Err(format!(
                "typed live transaction exceeds {MAX_LIVE_TRANSACTION_ASSIGNMENTS} assignments"
            ));
        }
    }
    if mutations.is_empty() {
        return Err("typed live transaction contains no assignments".to_string());
    }
    if !preview {
        for (path, _, value) in &mutations {
            jit.write_global_scalar(path, *value)?;
        }
    }
    Ok((
        if preview {
            "transaction_preview"
        } else {
            "transaction_committed"
        },
        json!({
            "preview": preview,
            "mutations": mutations.into_iter().map(|(path, old, new)| json!({
                "path": path, "static_type": old.type_name(), "old": old, "new": new
            })).collect::<Vec<_>>()
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn project() -> (PathBuf, LiveRunConfig) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_live_workspace_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("src");
        fs::create_dir_all(root.join("tests")).expect("tests");
        fs::write(
            root.join("src/main.stasis"),
            "global score: i32;\nfunction main(): i32 { score = 1; return 0; }\nfunction tick(): i32 { score += 1; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("source");
        fs::write(
            root.join("tests/main.test.stasis"),
            "test `live project remains valid`(): bool { return 1 == 1; }\n",
        )
        .expect("test");
        let config = LiveRunConfig::new(
            root.clone(),
            PathBuf::from("src/main.stasis"),
            PathBuf::from("build"),
        );
        (root, config)
    }

    fn compile(config: &LiveRunConfig) -> (JitProcess, JitEnginePackage) {
        let files =
            load_workshop_edit_workspace(&config.project_root, &config.entry).expect("files");
        let mut jit = JitProcess::new();
        jit.set_project_root(config.project_root.to_string_lossy())
            .expect("set project root");
        jit.set_required_emit_roots(&[
            "main".into(),
            "tick".into(),
            "render".into(),
            "on_code_swap".into(),
        ]);
        for file in files {
            jit.upsert_file(
                config.project_root.join(file.path).to_string_lossy(),
                file.source,
            );
        }
        jit.compile().expect("compile");
        let package = jit
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("package");
        (jit, package)
    }

    #[test]
    fn live_edit_batch_plans_all_symbols_as_one_transaction() {
        let (root, config) = project();
        let files = load_workshop_edit_workspace(&root, &config.entry).expect("files");
        let (after, plan) = plan_live_edit_batch(
            &files,
            vec![
                stasis_runner::live::LiveEdit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): i32 { score += 3; return 0; }".into()),
                    expected_source_hash: None,
                },
                stasis_runner::live::LiveEdit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "render".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function render(): i32 { return score; }".into()),
                    expected_source_hash: None,
                },
            ],
        )
        .expect("batch plan");
        assert_eq!(plan.edits.len(), 2);
        assert_eq!(plan.changed_files.len(), 1);
        let main = after
            .iter()
            .find(|file| file.path == "src/main.stasis")
            .expect("main");
        assert!(main.source.contains("score += 3"));
        assert!(main.source.contains("return score"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn compile_candidate_does_not_reload_imports_under_a_second_path() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "import \"adapter.stasis\";\nfunction main(): i32 { return helper(1); }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return helper(2); }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("main");
        fs::write(
            root.join("src/adapter.stasis"),
            "function helper(value: i32): i32 { return value; }\n",
        )
        .expect("adapter");
        let files = load_workshop_edit_workspace(&root, &config.entry).expect("files");

        compile_candidate(&config, &files, JitProcess::new())
            .expect("candidate with imported helper");

        fs::remove_dir_all(root).ok();
    }

    fn run_request(
        client: &stasis_runner::live::LiveSessionClient,
        workspace: &mut LiveWorkspace,
        jit: &mut JitProcess,
        tick_ptr: &mut u64,
        render_ptr: &mut u64,
        request: LiveRequest,
    ) -> LiveResponse {
        let request_id = request.request_id;
        client.submit(request).expect("submit live request");
        for tick in 1..=500 {
            workspace.process_boundary(tick, jit, tick_ptr, render_ptr);
            if let Ok(response) = client.receive_timeout(std::time::Duration::from_millis(10)) {
                if response.request_id == request_id
                    && !matches!(
                        response.kind.as_str(),
                        "edit_preparing" | "completion_preparing"
                    )
                {
                    return response;
                }
            }
        }
        panic!("live request {request_id} did not finish");
    }

    fn prepared_tick_edit(config: &LiveRunConfig, request_id: u64) -> PreparedEdit {
        let (active, _) = compile(config);
        let active_layout = active.state_layout();
        let candidate = active.staged_candidate();
        prepare_edit(
            request_id,
            config,
            EditPreparationInput::Edit {
                operation: LiveEditOperation::Update,
                target: LiveSymbolTarget {
                    name: "tick".into(),
                    kind: Some("function".into()),
                    file: Some("src/main.stasis".into()),
                    owner: None,
                    signature: None,
                },
                source: Some("function tick(): i32 { score += 4; return 0; }".into()),
                expected_source_hash: None,
                preview: false,
                run_tests: false,
            },
            &active_layout,
            candidate,
            &AtomicBool::new(false),
        )
        .expect("prepare edit")
    }

    fn install_ready_preparation(workspace: &mut LiveWorkspace, prepared: PreparedEdit) {
        let request_id = prepared.request_id;
        let canceled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Ok(prepared)).expect("queue prepared edit");
        workspace.edit_preparation = Some(EditPreparation {
            request_id,
            canceled,
            receiver,
            worker: None,
        });
    }

    #[test]
    fn live_commit_advances_from_external_watch_host_revision() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let initial_revision = stasis_dynload::jit_host_entry_targets()
            .map_or(1, |targets| targets.revision.saturating_add(1));
        stasis_dynload::begin_jit_host_entry_session(
            package
                .host_entry_targets(initial_revision)
                .expect("initial host targets"),
        )
        .expect("begin host-entry session");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");

        let external_revision = initial_revision.saturating_add(1);
        stasis_dynload::publish_jit_host_entry_targets(
            package
                .host_entry_targets(external_revision)
                .expect("external watch targets"),
        )
        .expect("publish external watch revision");
        workspace.refresh_after_external_edit(&jit);
        assert_eq!(workspace.host_entry_revision, external_revision);

        let prepared = prepared_tick_edit(&config, 701);
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        workspace
            .commit_prepared(prepared, &mut jit, &mut tick_ptr, &mut render_ptr)
            .expect("live commit after external watch publish");
        assert_eq!(
            stasis_dynload::jit_host_entry_targets()
                .expect("live targets")
                .revision,
            external_revision.saturating_add(1)
        );

        drop(client);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scalar_transactions_preview_and_commit_atomically() {
        let (root, config) = project();
        let (jit, _) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        apply_scalar_transaction(&jit, "score = 9;", true).expect("preview");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(1)));
        apply_scalar_transaction(&jit, "score = 9;", false).expect("commit");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(9)));
        assert!(apply_scalar_transaction(&jit, "score = nope;", false).is_err());
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(9)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn default_state_inspection_is_bounded_and_typed() {
        let (root, config) = project();
        let (jit, _) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (kind, data) = inspect_all_scalars(&jit, 32, false).expect("state inspection");
        assert_eq!(kind, "state_inspection");
        assert_eq!(data["total"], 1);
        assert_eq!(data["items"][0]["path"], "score");
        assert_eq!(data["items"][0]["static_type"], "i32");
        assert_eq!(data["items"][0]["value"]["value"], 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn concise_state_paths_hide_positions_and_proven_collection_metadata() {
        let paths = HashSet::from([
            "ball_x".to_string(),
            "ball_y".to_string(),
            "player.position.x".to_string(),
            "commands.length".to_string(),
            "commands.max_length".to_string(),
            "snake.length".to_string(),
            "score".to_string(),
        ]);
        for path in [
            "ball_x",
            "ball_y",
            "player.position.x",
            "commands.length",
            "commands.max_length",
        ] {
            assert!(!concise_state_path_is_visible(path, &paths), "{path}");
        }
        for path in ["snake.length", "score"] {
            assert!(concise_state_path_is_visible(path, &paths), "{path}");
        }
    }

    #[test]
    fn staged_test_output_drain_caps_diagnostics_and_flags_overflow() {
        let bytes = vec![b'x'; MAX_STAGED_TEST_OUTPUT_BYTES + 1];
        let total = AtomicUsize::new(0);
        let overflow = AtomicBool::new(false);
        let captured = drain_bounded_test_output(std::io::Cursor::new(bytes), &total, &overflow)
            .expect("drain output");
        assert_eq!(captured.len(), MAX_STAGED_TEST_DIAGNOSTIC_BYTES);
        assert_eq!(
            total.load(Ordering::Acquire),
            MAX_STAGED_TEST_OUTPUT_BYTES + 1
        );
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    fn live_runtime_candidate_excludes_test_only_symbols() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global score: i32;\nfunction helper(): i32 { return 7; }\nfunction main(): i32 { score = 0; return 0; }\nfunction tick(): i32 { score = helper(); return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("main source");
        fs::write(
            root.join("tests/main.test.stasis"),
            "function helper(): i32 { return 99; }\ntest `helper stays isolated`(): bool { return helper() == 99; }\n",
        )
        .expect("test-only helper");

        let files = load_workshop_edit_workspace(&root, &config.entry).expect("workspace files");
        let (jit, package) =
            compile_candidate(&config, &files, JitProcess::new()).expect("runtime candidate");
        jit.activate_staged_runtime().expect("activate candidate");
        jit.execute_i32_noarg_by_name("main").expect("main");
        stasis_dynload::invoke_noarg_i32(package.tick_code_ptr as usize).expect("tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(7)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reference_request_returns_compact_containing_symbols() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                39,
                LiveCommand::References {
                    symbol: "score".into(),
                    limit: 16,
                },
            ),
        );

        assert!(response.ok);
        let references = response.data.expect("reference data")["references"]
            .as_array()
            .expect("references")
            .clone();
        assert!(references
            .iter()
            .any(|reference| reference["containing_name"] == "tick"));
        assert!(references
            .iter()
            .all(|reference| reference.get("source").is_none()));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn symbol_search_is_filtered_compact_and_hash_free() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "import \"helper.stasis\";\nglobal score: i32;\nfunction main(): i32 { score = 1; return 0; }\nfunction tick(): i32 { score += 1; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("source with import");
        fs::write(
            root.join("src/helper.stasis"),
            "function direct_import_value(): i32 { return 1; }\n",
        )
        .expect("helper source");
        let (jit, _package) = compile(&config);
        let (_client, server) = stasis_runner::live::live_session(8);
        let workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");

        let (_, all) = workspace
            .symbols(None, None, &[], None, 0, 32)
            .expect("symbols");
        let items = all["items"].as_array().expect("items");
        assert!(items.iter().all(|item| item["kind"] != "imports"));
        assert!(items.iter().all(|item| item.get("source_hash").is_none()));
        assert!(items.iter().all(|item| item.get("source").is_none()));
        assert!(items
            .iter()
            .all(|item| { matches!(item.as_object().map(|object| object.len()), Some(4 | 5)) }));
        assert_eq!(
            all["files"],
            json!(["src/helper.stasis", "src/main.stasis"])
        );
        assert_eq!(
            all["imports"],
            json!({"src/helper.stasis": [], "src/main.stasis": ["src/helper.stasis"]})
        );
        assert!(items
            .iter()
            .any(|item| item["name"] == "direct_import_value"));

        let (_, filtered) = workspace
            .symbols(
                Some("tick"),
                Some("function"),
                &["src/main.stasis".to_string()],
                None,
                0,
                1,
            )
            .expect("filtered symbols");
        assert_eq!(filtered["total"], 1);
        assert_eq!(filtered["items"][0]["name"], "tick");
        assert!(filtered["items"][0].get("owner").is_none());

        let (_, tests) = workspace
            .symbols(
                None,
                Some("test"),
                &["tests/main.test.stasis".to_string()],
                None,
                0,
                32,
            )
            .expect("test symbols");
        assert_eq!(tests["total"], 1);
        assert_eq!(tests["files"], json!(["tests/main.test.stasis"]));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn validation_snapshot_restores_the_same_runtime_baseline() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;

        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(40, LiveCommand::ValidationSnapshot),
            )
            .ok
        );
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    41,
                    LiveCommand::Set {
                        path: "score".into(),
                        expression: "9".into(),
                        preview: false,
                    },
                ),
            )
            .ok
        );
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(9)));
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(42, LiveCommand::ValidationRestore),
            )
            .ok
        );
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(1)));
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(43, LiveCommand::ValidationClear),
            )
            .ok
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn human_runtime_validation_restores_live_state_after_frames() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                44,
                LiveCommand::Validate {
                    requirement: stasis_runner::live::LiveValidationRequirement {
                        path: "score".into(),
                        op: "eq".into(),
                        value: json!(3),
                    },
                    frames: 2,
                },
            ),
        );

        assert!(response.ok);
        assert_eq!(response.data.expect("validation")["requirements_met"], true);
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(1)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn semantic_noop_edit_is_rejected_without_writing() {
        let plan = WorkshopSemanticEditPlan {
            schema_version: 1,
            edits: Vec::new(),
            changed_files: Vec::new(),
            reload: stasis_compiler::frontend::workshop::WorkshopReloadClassification {
                expected_reload: ExpectedReload::FastReload,
                reason: "No symbol changes detected.".into(),
                changed_symbols: Vec::new(),
            },
        };
        let error = require_semantic_changes(&plan).expect_err("semantic no-op");

        assert!(error.contains("disk and runtime were left unchanged"));
    }

    #[test]
    fn pause_step_and_expression_watch_events_are_boundary_exact() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    40,
                    LiveCommand::Watch {
                        path: "score + 1".into()
                    }
                ),
            )
            .ok
        );
        workspace.publish_watches(1, &jit);
        assert!(client
            .receive_timeout(std::time::Duration::from_millis(10))
            .is_err());

        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    41,
                    LiveCommand::Set {
                        path: "score".into(),
                        expression: "9".into(),
                        preview: false,
                    },
                ),
            )
            .ok
        );
        workspace.publish_watches(1, &jit);
        let watch = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("watch event");
        assert_eq!(watch.request_id, 0);
        assert_eq!(watch.tick, 1);
        assert_eq!(watch.data.expect("watch data")["value"]["value"], 10);
        workspace.publish_watches(1, &jit);
        assert!(client
            .receive_timeout(std::time::Duration::from_millis(10))
            .is_err());

        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(42, LiveCommand::Pause),
            )
            .ok
        );
        assert!(!workspace.should_run_tick());
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(43, LiveCommand::Step { ticks: 2 }),
            )
            .ok
        );
        assert!(workspace.should_run_tick());
        workspace.after_tick();
        assert!(workspace.should_run_tick());
        workspace.after_tick();
        assert!(!workspace.should_run_tick());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn expression_watch_reports_and_deduplicates_evaluation_errors() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    43,
                    LiveCommand::Watch {
                        path: "10 / score".into(),
                    },
                ),
            )
            .ok
        );
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    44,
                    LiveCommand::Set {
                        path: "score".into(),
                        expression: "0".into(),
                        preview: false,
                    },
                ),
            )
            .ok
        );

        workspace.publish_watches(2, &jit);
        let error = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("watch error");
        assert_eq!(error.kind, "watch_error");
        assert!(error.data.expect("watch error data")["error"]
            .as_str()
            .unwrap_or_default()
            .contains("division by zero"));
        workspace.publish_watches(3, &jit);
        assert!(client
            .receive_timeout(std::time::Duration::from_millis(10))
            .is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn predicate_watches_share_one_per_tick_scan_budget() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "struct Enemy { hp: i32; }\n\
             global alpha: Enemy[4096];\n\
             global beta: Enemy[4096];\n\
             function main(): i32 { return alpha[0].hp + beta[0].hp; }\n\
             function tick(): i32 { return 0; }\n\
             function render(): i32 { return 0; }\n\
             function on_code_swap(): void { return; }\n",
        )
        .expect("predicate watch source");
        let (jit, _) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        workspace.watches.insert("alpha[?hp >= 0]".into(), None);
        workspace.watches.insert("beta[?hp >= 0]".into(), None);

        workspace.publish_watches(1, &jit);

        let scanned = (0..2)
            .map(|_| {
                client
                    .receive_timeout(std::time::Duration::from_secs(1))
                    .expect("predicate watch event")
                    .data
                    .expect("predicate watch data")["inspection"]["scanned"]
                    .as_u64()
                    .unwrap_or(0)
            })
            .sum::<u64>();
        assert_eq!(scanned, MAX_WATCH_PREDICATE_SCAN_PER_TICK as u64);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn code_aware_edit_preview_apply_and_undo_preserve_runtime_and_disk() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let target = LiveSymbolTarget {
            name: "tick".into(),
            kind: Some("function".into()),
            file: Some("src/main.stasis".into()),
            owner: None,
            signature: None,
        };
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                1,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target,
                    source: Some("function tick(): i32 { score += 4; return 0; }".into()),
                    expected_source_hash: None,
                    preview: true,
                    run_tests: false,
                },
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("score += 1"));

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(2, LiveCommand::Apply { run_tests: false }),
        );
        assert!(response.ok, "{:?}", response.error);
        let patch = &response.data.as_ref().expect("apply data")["jit_patch"];
        assert_eq!(patch["re_jit_count"], 1);
        assert_eq!(patch["reused_count"], 3);
        assert!(patch["revision"]
            .as_u64()
            .is_some_and(|revision| revision > 0));
        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("new tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(5)));
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("score += 4"));

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(3, LiveCommand::Undo { run_tests: false }),
        );
        assert!(response.ok, "{:?}", response.error);
        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("old tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(6)));
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("score += 1"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn layout_edit_previews_then_preserves_state_and_initializes_new_field() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                1,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Add,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global extra: i32;".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.kind, "edit_preview");
        let data = response.data.expect("preview data");
        assert_eq!(data["validated"], true);
        assert_eq!(data["swap"]["layout_changed"], true);
        assert_eq!(data["swap"]["requires_explicit_apply"], true);
        assert_eq!(data["swap"]["migration_scope"]["kind"], "whole_state");
        let changed_functions = data["swap"]["changed_functions"]
            .as_array()
            .expect("canonical changed function identities");
        assert!(changed_functions.iter().all(|identity| identity
            .as_str()
            .is_some_and(|identity| identity.starts_with("v1|function|"))));
        assert!(data["swap"]["migration_steps"]
            .as_array()
            .expect("migration steps")
            .iter()
            .any(|step| step["kind"] == "initialize" && step["path"] == "extra"));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );

        let applied = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(2, LiveCommand::Apply { run_tests: false }),
        );
        assert!(applied.ok, "{:?}", applied.error);
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(1)));
        assert_eq!(jit.read_global_scalar("extra"), Ok(JitScalarValue::I32(0)));
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("global extra: i32;"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn collection_capacity_shrink_warns_and_copies_only_retained_elements() {
        let (root, config) = project();
        let sample = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("samples/layout_migration_preview.stasis");
        fs::write(
            root.join("src/main.stasis"),
            fs::read_to_string(sample).expect("migration sample"),
        )
        .expect("array source");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                10,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global score: i32;\nglobal values: i32[2];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        let swap = &response.data.expect("preview")["swap"];
        assert!(swap["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .is_some_and(|text| text.contains("shrinks from 4 to 2"))));

        let applied = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(11, LiveCommand::Apply { run_tests: false }),
        );
        assert!(applied.ok, "{:?}", applied.error);
        assert_eq!(
            jit.read_global_collection_scalar("values", "", 0),
            Ok(JitScalarValue::I32(11))
        );
        assert_eq!(
            jit.read_global_collection_scalar("values", "", 1),
            Ok(JitScalarValue::I32(22))
        );
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(7)));
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("migrated tick"),
            19
        );
        assert!(jit
            .read_global_collection_scalar("values", "", 2)
            .expect_err("new capacity should reject index 2")
            .contains("outside capacity 2"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn collection_capacity_growth_preserves_prefix_and_initializes_tail() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global values: i32[2];\nfunction main(): i32 { values[0] = 11; values[1] = 22; return 0; }\nfunction tick(): i32 { return values[0] + values[1]; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("array source");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                12,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global values: i32[4];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        assert!(preview.data.expect("preview")["swap"]["migration_steps"]
            .as_array()
            .expect("steps")
            .iter()
            .any(|step| {
                step["kind"] == "initialize" && step["start_index"] == 2 && step["elements"] == 2
            }));

        let applied = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(13, LiveCommand::Apply { run_tests: false }),
        );
        assert!(applied.ok, "{:?}", applied.error);
        assert_eq!(
            jit.read_global_collection_scalar("values", "", 0),
            Ok(JitScalarValue::I32(11))
        );
        assert_eq!(
            jit.read_global_collection_scalar("values", "", 1),
            Ok(JitScalarValue::I32(22))
        );
        assert_eq!(
            jit.read_global_collection_scalar("values", "", 2),
            Ok(JitScalarValue::I32(0))
        );
        jit.write_global_collection_scalar("values", "", 3, JitScalarValue::I32(44))
            .expect("expanded storage accepts the new tail");
        assert_eq!(
            jit.read_global_collection_scalar("values", "", 3),
            Ok(JitScalarValue::I32(44))
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn collection_growth_preview_rejects_host_owned_storage() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global values: i32[2];\nfunction main(): i32 { return 0; }\nfunction tick(): i32 { return values[0]; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("array source");
        let (mut jit, package) = compile(&config);
        let mut host_values = vec![11, 22];
        stasis_dynload::register_global_i32_array(
            crate::hash_global_path("values"),
            0,
            host_values.as_mut_ptr(),
            host_values.len(),
        );
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                18,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global values: i32[4];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        let data = preview.data.expect("preview");
        assert_eq!(data["validated"], false);
        assert!(data["swap"]["rejection"]
            .as_str()
            .is_some_and(|error| error.contains("host-owned")));
        assert_eq!(host_values, [11, 22]);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn collection_growth_preview_rejects_unbounded_allocation() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global values: i32[2];\nfunction main(): i32 { return 0; }\nfunction tick(): i32 { return values[0]; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("array source");
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                19,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global values: i32[3000000];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        let data = preview.data.expect("preview");
        assert_eq!(data["validated"], false);
        assert!(data["swap"]["rejection"]
            .as_str()
            .is_some_and(|error| error.contains("8") && error.contains("live limit")));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn new_collection_commit_allocates_and_initializes_storage() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global score: i32;\nfunction main(): i32 { score = 7; return 0; }\nfunction tick(): i32 { return score; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("scalar source");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                20,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global score: i32;\nglobal fresh_values: i32[4];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        assert_eq!(preview.data.expect("preview")["validated"], true);

        let applied = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(21, LiveCommand::Apply { run_tests: false }),
        );
        assert!(applied.ok, "{:?}", applied.error);
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(7)));
        assert_eq!(
            jit.read_global_collection_scalar("fresh_values", "", 3),
            Ok(JitScalarValue::I32(0))
        );
        jit.write_global_collection_scalar("fresh_values", "", 3, JitScalarValue::I32(44))
            .expect("new storage accepts writes");
        assert_eq!(
            jit.read_global_collection_scalar("fresh_values", "", 3),
            Ok(JitScalarValue::I32(44))
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn new_collection_preview_rejects_unbounded_allocation() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global score: i32;\nfunction main(): i32 { return 0; }\nfunction tick(): i32 { return score; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("scalar source");
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                22,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global score: i32;\nglobal huge_values: i32[3000000];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        let data = preview.data.expect("preview");
        assert_eq!(data["validated"], false);
        assert!(data["swap"]["rejection"]
            .as_str()
            .is_some_and(|error| error.contains("8") && error.contains("live limit")));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn text_capacity_shrink_copies_bytes_and_clamps_lengths() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global label: ascii[4];\nglobal glyphs: utf8[3];\nglobal word: utf8[3];\nfunction main(): i32 { label[0] = 65; label[1] = 66; label[2] = 67; label.length = 4; label.byte_length = 4; glyphs[0] = 195; glyphs[1] = 169; glyphs[2] = 65; glyphs.length = 3; glyphs.byte_length = 3; glyphs.char_length = 2; word[0] = 195; word[1] = 169; word[2] = 65; word.length = 3; word.char_length = 2; return 0; }\nfunction tick(): i32 { return label.length + glyphs.length + word.length; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("text source");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                14,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some(
                        "global label: ascii[2];\nglobal glyphs: utf8[1];\nglobal word: utf8[2];"
                            .into(),
                    ),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        let preview_data = preview.data.expect("preview");
        let swap = &preview_data["swap"];
        let warnings = swap["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert!(
            warnings.iter().any(|warning| warning.contains("label[]")),
            "{swap:#}\nactive={:#?}",
            jit.state_layout()
        );
        assert!(
            warnings.iter().any(|warning| warning.contains("glyphs[]")),
            "{swap:#}\nactive={:#?}",
            jit.state_layout()
        );
        assert!(warnings.iter().any(|warning| warning.contains("word[]")));

        let applied = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(15, LiveCommand::Apply { run_tests: false }),
        );
        assert!(applied.ok, "{:?}", applied.error);
        assert_eq!(
            jit.read_global_scalar("label.length"),
            Ok(JitScalarValue::I32(2))
        );
        assert_eq!(
            jit.read_global_scalar("glyphs.length"),
            Ok(JitScalarValue::I32(0))
        );
        assert_eq!(
            jit.read_global_scalar("label.byte_length"),
            Ok(JitScalarValue::I32(2))
        );
        assert_eq!(
            jit.read_global_scalar("glyphs.byte_length"),
            Ok(JitScalarValue::I32(0))
        );
        assert_eq!(
            jit.read_global_scalar("glyphs.char_length"),
            Ok(JitScalarValue::I32(0))
        );
        assert_eq!(
            jit.read_global_collection_scalar("label", "", 0),
            Ok(JitScalarValue::U8(65))
        );
        assert_eq!(
            jit.read_global_collection_scalar("glyphs", "", 1),
            Err("global collection path 'glyphs' index 1 is outside capacity 1".to_string())
        );
        assert_eq!(
            jit.read_global_collection_scalar("glyphs", "", 0),
            Ok(JitScalarValue::U8(0))
        );
        assert_eq!(
            jit.read_global_scalar("word.length"),
            Ok(JitScalarValue::I32(2))
        );
        assert_eq!(
            jit.read_global_scalar("word.byte_length"),
            Ok(JitScalarValue::I32(2))
        );
        assert_eq!(
            jit.read_global_scalar("word.char_length"),
            Ok(JitScalarValue::I32(1))
        );
        assert_eq!(
            jit.read_global_collection_scalar("word", "", 0),
            Ok(JitScalarValue::U8(195))
        );
        assert_eq!(
            jit.read_global_collection_scalar("word", "", 1),
            Ok(JitScalarValue::U8(169))
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hook_rejection_rolls_back_hook_mutation_code_and_disk() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "extern function reject_code_swap(): void;\nglobal score: i32;\nfunction main(): i32 { score = 7; return 0; }\nfunction tick(): i32 { return score; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("hook source");
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let old_tick_ptr = package.tick_code_ptr;
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                16,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "on_code_swap".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some(
                        "function on_code_swap(): void { score = 99; reject_code_swap(); return; }"
                            .into(),
                    ),
                    expected_source_hash: None,
                    preview: true,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        let rejected = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(17, LiveCommand::Apply { run_tests: false }),
        );
        assert!(!rejected.ok);
        assert!(rejected
            .error
            .as_deref()
            .is_some_and(|error| error.contains("hook requested rejection")));
        assert_eq!(tick_ptr, old_tick_ptr);
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(7)));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hook_rejection_after_growth_restores_old_collection_registration() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "extern function reject_code_swap(): void;\nglobal rollback_values: i32[2];\nfunction main(): i32 { rollback_values[0] = 11; rollback_values[1] = 22; return 0; }\nfunction tick(): i32 { return rollback_values[0] + rollback_values[1]; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { rollback_values[0] = 99; reject_code_swap(); return; }\n",
        )
        .expect("hook source");
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                22,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global rollback_values: i32[4];".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        let rejected = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(23, LiveCommand::Apply { run_tests: false }),
        );
        assert!(!rejected.ok);
        assert_eq!(
            jit.read_global_collection_scalar("rollback_values", "", 0),
            Ok(JitScalarValue::I32(11))
        );
        assert_eq!(
            jit.read_global_collection_scalar("rollback_values", "", 1),
            Ok(JitScalarValue::I32(22))
        );
        assert!(jit
            .read_global_collection_scalar("rollback_values", "", 2)
            .expect_err("old capacity restored")
            .contains("outside capacity 2"));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn incompatible_state_type_preview_cannot_commit() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global score: i32;\nglobal spare: i32;\nfunction main(): i32 { score = 7; return 0; }\nfunction tick(): i32 { return score; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("source");
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                20,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "globals".into(),
                        kind: Some("globals".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("global score: i32;\nglobal spare: f32;".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        let data = response.data.expect("preview");
        assert_eq!(data["validated"], false);
        assert!(data["swap"]["rejection"]
            .as_str()
            .is_some_and(|text| text.contains("spare")
                && text.contains("i32")
                && text.contains("f32")));

        let rejected = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(21, LiveCommand::Apply { run_tests: false }),
        );
        assert!(!rejected.ok);
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(7)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn code_aware_add_delete_refreshes_completion_and_rejects_stale_hash() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config.clone(), &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let target = LiveSymbolTarget {
            name: "helper".into(),
            kind: Some("function".into()),
            file: Some("src/main.stasis".into()),
            owner: None,
            signature: None,
        };
        let added = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                10,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Add,
                    target: target.clone(),
                    source: Some(
                        "function helper(value: i32): i32 { let local: i32 = value; return local; }"
                            .into(),
                    ),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(added.ok, "{:?}", added.error);
        assert_eq!(
            workspace.completion.complete(":read hel", 9, 10)[0].text,
            "helper"
        );
        let helper_context = stasis_runner::live::CompletionContext {
            owner: Some("helper".into()),
            file: Some("src/main.stasis".into()),
            ..stasis_runner::live::CompletionContext::default()
        };
        assert_eq!(
            workspace
                .completion
                .query_with_context("lcl", 3, 10, &helper_context)
                .items[0]
                .text,
            "local"
        );
        assert!(workspace.consumes_self_write(&root.join("src/main.stasis")));

        let stale = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                11,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: target.clone(),
                    source: Some(
                        "function helper(value: i32): i32 { let local: i32 = value; return 8; }"
                            .into(),
                    ),
                    expected_source_hash: Some("stale".into()),
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(!stale.ok);
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("return local"));

        let files = load_workshop_edit_workspace(&root, &config.entry).expect("files");
        let helper = find_workshop_symbols(&files, &selector(&target).expect("selector"))
            .expect("symbols")
            .remove(0);
        let deleted = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                12,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Delete,
                    target,
                    source: None,
                    expected_source_hash: Some(helper.source_hash),
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(deleted.ok, "{:?}", deleted.error);
        assert!(workspace
            .completion
            .complete(":read hel", 9, 10)
            .iter()
            .all(|item| item.text != "helper"));
        assert!(workspace
            .completion
            .query_with_context("lcl", 3, 10, &helper_context)
            .items
            .iter()
            .all(|item| item.text != "local"));
        assert!(!fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("function helper"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dirty_unbalanced_definition_overlay_completes_new_typed_local() {
        let (root, config) = project();
        let (jit, _) = compile(&config);
        let (_, server) = stasis_runner::live::live_session(8);
        let workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let tick = workspace
            .source_items
            .iter()
            .find(|item| item.name == "tick")
            .expect("tick item");
        let buffer = "function tick(): i32 {\n    let local_speed: i32 = 2;\n    local_sp";
        let context = stasis_runner::live::CompletionContext {
            owner: Some("tick".into()),
            file: Some(tick.file.clone()),
            owner_signature: Some(tick.signature.clone()),
            source_offset: Some(tick.source_spans[0].start as usize + buffer.len()),
            expected_type: None,
        };
        let query = workspace.completion_query(buffer, buffer.len(), 10, &context);
        assert_eq!(query.items[0].text, "local_speed");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dirty_document_overlay_infers_scope_and_completes_new_local() {
        let (root, config) = project();
        let (jit, _) = compile(&config);
        let (_, server) = stasis_runner::live::live_session(8);
        let workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let source = fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .replace(
                "function tick(): i32 { score += 1; return 0; }",
                "function tick(): i32 {\n    let local_speed: i32 = 2;\n    local_sp\n}",
            );
        let cursor = source.rfind("local_sp").expect("completion prefix") + "local_sp".len();
        let context = CompletionContext {
            owner: None,
            file: Some("src/main.stasis".into()),
            owner_signature: None,
            source_offset: Some(cursor),
            expected_type: None,
        };

        let query = workspace.completion_query(&source, cursor, 10, &context);
        let local = query
            .items
            .iter()
            .find(|item| item.text == "local_speed")
            .expect("dirty local completion");
        let scope = local.scope.as_ref().expect("dirty local scope");
        assert_eq!(scope.owner, "tick");
        assert_eq!(scope.owner_signature.as_deref(), Some("tick(): i32"));
        assert!(
            scope.visible_from <= cursor && cursor <= scope.visible_to,
            "{scope:?}"
        );

        assert_eq!(query.items[0].text, "local_speed");
        assert_eq!(query.replacement_end, cursor);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn static_type_fields_are_hidden_at_root_and_available_while_editing() {
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "struct Player { hp: i32; }\nstruct Enemy { hp: i32; }\nstruct Game { enemies: Enemy[2]; }\nglobal player: Player;\nglobal game: Game;\nfunction main(): i32 { player.hp = 7; game.enemies[0].hp = 37; return 0; }\nfunction tick(): i32 { return player.hp; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("source");
        let (jit, _) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("run main");
        let (kind, evaluation) =
            evaluate_expression(&jit, "game.enemies[0].hp").expect("evaluate hp");
        assert_eq!(kind, "evaluation");
        assert_eq!(evaluation["static_type"], "i32");
        assert_eq!(evaluation["value"]["value"], 37);
        let (_, server) = stasis_runner::live::live_session(8);
        let workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");

        let root_query = workspace.completion.query("Player.h", 8, 10);
        assert!(root_query.items.iter().all(|item| item.text != "Player.hp"));
        assert_eq!(
            workspace.completion.query("player.h", 8, 10).items[0].text,
            "player.hp"
        );

        let tick = workspace
            .source_items
            .iter()
            .find(|item| item.name == "tick")
            .expect("tick item");
        let buffer = "function tick(): i32 { return Player.h";
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some(tick.file.clone()),
            owner_signature: Some(tick.signature.clone()),
            source_offset: Some(tick.source_spans[0].start as usize + buffer.len()),
            expected_type: None,
        };
        let edit_query = workspace.completion_query(buffer, buffer.len(), 10, &context);
        assert!(edit_query.items.iter().any(|item| item.text == "Player.hp"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dirty_overlay_removes_deleted_locals_from_the_accepted_catalog() {
        let (root, config) = project();
        let path = root.join("src/main.stasis");
        let source = fs::read_to_string(&path).expect("source").replace(
            "function tick(): i32 { score += 1; return 0; }",
            "function tick(): i32 { let stale_local: i32 = 1; score += stale_local; return 0; }",
        );
        fs::write(&path, source).expect("write local source");
        let (jit, _) = compile(&config);
        let (_, server) = stasis_runner::live::live_session(8);
        let workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let tick = workspace
            .source_items
            .iter()
            .find(|item| item.name == "tick")
            .expect("tick item");
        let buffer = "function tick(): i32 {\n    score += 1;\n    stale";
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some(tick.file.clone()),
            owner_signature: Some(tick.signature.clone()),
            source_offset: Some(tick.source_spans[0].start as usize + buffer.len()),
            expected_type: None,
        };
        let query = workspace.completion_query(buffer, buffer.len(), 10, &context);
        assert!(query.items.iter().all(|item| item.text != "stale_local"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dirty_overlay_keeps_scope_identity_when_a_parameter_is_renamed() {
        let (root, config) = project();
        let path = root.join("src/main.stasis");
        let mut source = fs::read_to_string(&path).expect("source");
        source.push_str("function helper(old_name: i32): i32 { return old_name; }\n");
        fs::write(&path, source).expect("write helper source");
        let (jit, _) = compile(&config);
        let (_, server) = stasis_runner::live::live_session(8);
        let workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let helper = workspace
            .source_items
            .iter()
            .find(|item| item.name == "helper")
            .expect("helper item");
        let buffer = "function helper(new_name: i32): i32 { return new_na";
        let context = CompletionContext {
            owner: Some("helper".into()),
            file: Some(helper.file.clone()),
            owner_signature: Some(helper.signature.clone()),
            source_offset: Some(helper.source_spans[0].start as usize + buffer.len()),
            expected_type: None,
        };
        let query = workspace.completion_query(buffer, buffer.len(), 10, &context);
        assert_eq!(query.items[0].text, "new_name");
        assert!(query.items.iter().all(|item| item.text != "old_name"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn completion_analysis_returns_preparing_before_the_background_result() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let tick = workspace
            .source_items
            .iter()
            .find(|item| item.name == "tick")
            .expect("tick item");
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some(tick.file.clone()),
            owner_signature: Some(tick.signature.clone()),
            source_offset: Some(tick.source_spans[0].start as usize),
            expected_type: None,
        };
        let buffer = "function tick(): i32 { sco".to_string();
        let cursor = buffer.len();
        client
            .submit(LiveRequest::new(
                90,
                LiveCommand::Complete {
                    buffer,
                    cursor,
                    limit: 10,
                    context,
                },
            ))
            .expect("submit completion");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        workspace.process_boundary(1, &mut jit, &mut tick_ptr, &mut render_ptr);
        let preparing = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("preparing response");
        assert_eq!(preparing.kind, "completion_preparing");
        let final_response = (2..=100)
            .find_map(|boundary| {
                workspace.process_boundary(boundary, &mut jit, &mut tick_ptr, &mut render_ptr);
                client
                    .receive_timeout(std::time::Duration::from_millis(10))
                    .ok()
            })
            .expect("background completion response");
        assert_eq!(final_response.kind, "completion");
        assert!(final_response.ok);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn watch_paths_are_bounded() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        workspace.watches = (0..MAX_LIVE_WATCHES)
            .map(|index| (format!("placeholder_{index}"), None))
            .collect();
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                91,
                LiveCommand::Watch {
                    path: "score".into(),
                },
            ),
        );
        assert!(!response.ok);
        assert!(response.error.expect("watch limit").contains("limited"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn large_palette_query_stays_bounded_and_completes_in_one_graphics_boundary() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut items = (0..10_000)
            .map(|index| CompletionItem {
                text: format!("generated_{index:05}"),
                kind: "function".into(),
                detail: format!("generated_{index:05}(): i32"),
                type_name: Some("i32".into()),
                source: Some("src/generated.stasis".into()),
                selector: None,
                scope: None,
            })
            .collect::<Vec<_>>();
        items.push(CompletionItem {
            text: "late_graphics_target".into(),
            kind: "function".into(),
            detail: "late_graphics_target(): i32".into(),
            type_name: Some("i32".into()),
            source: Some("src/late.stasis".into()),
            selector: None,
            scope: None,
        });
        workspace.completion.replace(items);
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                13,
                LiveCommand::Palette {
                    query: "lgt".into(),
                    page: 0,
                    limit: 8,
                    context: stasis_runner::live::CompletionContext::default(),
                },
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.tick, 1);
        assert_eq!(
            response.data.expect("palette")["items"][0]["text"],
            "late_graphics_target"
        );
        let page = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                14,
                LiveCommand::Palette {
                    query: "generated".into(),
                    page: 2,
                    limit: 8,
                    context: stasis_runner::live::CompletionContext::default(),
                },
            ),
        );
        assert_eq!(
            page.data.expect("page")["items"][0]["text"],
            "generated_00016"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn receipt_failure_rolls_back_disk_dispatch_and_state() {
        let (root, mut config) = project();
        config.output = PathBuf::from("receipt-blocker");
        fs::write(root.join("receipt-blocker"), "not a directory").expect("block receipt");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                20,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): i32 { score += 4; return 0; }".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(!response.ok);
        assert!(response.error.expect("error").contains("receipt"));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        assert!(workspace.consumes_self_write(&root.join("src/main.stasis")));
        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("old tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(2)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn compiler_failure_leaves_disk_dispatch_and_state_unchanged() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                25,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): i32 { missing syntax }".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(!response.ok);
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("old tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(2)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cached_browse_disambiguates_same_name_overloads_and_completion_keeps_both() {
        let (root, config) = project();
        fs::write(
            root.join("src/overloads.stasis"),
            "function convert(value: i32): i32 { return value; }\nfunction convert(value: f32): f32 { return value; }\n",
        )
        .expect("overloads");
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let overloads = workspace
            .source_items
            .iter()
            .filter(|item| item.name == "convert")
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(overloads.len(), 2);
        assert_eq!(workspace.completion.complete(":read con", 9, 10).len(), 2);
        for (request_id, overload) in (50..).zip(overloads) {
            let response = run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    request_id,
                    LiveCommand::Read {
                        name: "convert".into(),
                        kind: Some("function".into()),
                        file: Some("src/overloads.stasis".into()),
                        owner: None,
                        signature: Some(overload.signature.clone()),
                    },
                ),
            );
            assert!(response.ok, "{:?}", response.error);
            assert_eq!(
                response.data.expect("symbol")["signature"],
                overload.signature
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn background_edit_preparation_keeps_status_responsive() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        client
            .submit(LiveRequest::new(
                30,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): i32 { score += 2; return 0; }".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: true,
                },
            ))
            .expect("edit");
        client
            .submit(LiveRequest::new(31, LiveCommand::Status))
            .expect("status");
        workspace.process_boundary(1, &mut jit, &mut tick_ptr, &mut render_ptr);
        let preparing = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("preparing");
        let status = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("status");
        assert_eq!(preparing.kind, "edit_preparing");
        assert_eq!(status.kind, "status");
        assert_eq!(status.data.expect("data")["preparing_request_id"], 30);
        client
            .submit(LiveRequest::new(32, LiveCommand::Cancel { request_id: 30 }))
            .expect("cancel");
        workspace.process_boundary(2, &mut jit, &mut tick_ptr, &mut render_ptr);
        let cancellation = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("cancellation acknowledgement");
        assert_eq!(cancellation.request_id, 32);
        assert!(cancellation.ok);
        assert_eq!(cancellation.data.expect("cancel data")["background"], true);
        let mut canceled = false;
        for tick in 3..=500 {
            workspace.process_boundary(tick, &mut jit, &mut tick_ptr, &mut render_ptr);
            if let Ok(response) = client.receive_timeout(std::time::Duration::from_millis(10)) {
                if response.request_id == 30 && response.kind != "edit_preparing" {
                    canceled = !response.ok;
                    break;
                }
            }
        }
        assert!(canceled);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn queued_cancel_wins_over_a_ready_background_commit() {
        let (root, config) = project();
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let prepared = prepared_tick_edit(&config, 50);
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(32);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        install_ready_preparation(&mut workspace, prepared);
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        for request_id in 100..109 {
            client
                .submit(LiveRequest::new(request_id, LiveCommand::Status))
                .expect("queued status");
        }
        client
            .submit(LiveRequest::new(51, LiveCommand::Cancel { request_id: 50 }))
            .expect("cancel");

        workspace.process_boundary(1, &mut jit, &mut tick_ptr, &mut render_ptr);

        let canceled = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("canceled edit");
        let acknowledgement = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("cancel acknowledgement");
        assert_eq!(canceled.request_id, 50);
        assert!(!canceled.ok);
        assert_eq!(acknowledgement.request_id, 51);
        assert_eq!(acknowledgement.data.expect("data")["background"], true);
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        for request_id in 100..108 {
            let response = client
                .receive_timeout(std::time::Duration::from_secs(1))
                .expect("status response");
            assert_eq!(response.request_id, request_id);
        }
        workspace.process_boundary(2, &mut jit, &mut tick_ptr, &mut render_ptr);
        let final_status = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("final status");
        assert_eq!(final_status.request_id, 108);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sustained_request_refill_keeps_internal_backlog_bounded() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(MAX_PENDING_LIVE_REQUESTS);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let mut request_id = 1000u64;
        let mut saw_backpressure = false;

        for tick in 1..=10 {
            for _ in 0..MAX_PENDING_LIVE_REQUESTS {
                client
                    .submit(LiveRequest::new(request_id, LiveCommand::Status))
                    .expect("bounded request submit");
                request_id += 1;
            }
            workspace.process_boundary(tick, &mut jit, &mut tick_ptr, &mut render_ptr);
            assert!(workspace.pending_requests.len() <= MAX_PENDING_LIVE_REQUESTS);
            while let Ok(response) = client.receive_timeout(std::time::Duration::from_millis(1)) {
                saw_backpressure |= response
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("backpressure"));
            }
        }
        assert!(saw_backpressure);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unrelated_source_change_rejects_a_ready_background_commit() {
        let (root, config) = project();
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let prepared = prepared_tick_edit(&config, 60);
        fs::write(
            root.join("tests/main.test.stasis"),
            "test `changed during preparation`(): bool { return 2 == 2; }\n",
        )
        .expect("change unrelated test input");
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        install_ready_preparation(&mut workspace, prepared);
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;

        workspace.process_boundary(1, &mut jit, &mut tick_ptr, &mut render_ptr);

        let response = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("stale rejection");
        assert_eq!(response.request_id, 60);
        assert!(!response.ok);
        assert!(response
            .error
            .expect("error")
            .contains("changed during background preparation"));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancellation_in_same_boundary_prevents_command_execution() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        client
            .submit(LiveRequest::new(1, LiveCommand::Pause))
            .expect("pause");
        client
            .submit(LiveRequest::new(2, LiveCommand::Cancel { request_id: 1 }))
            .expect("cancel");
        workspace.process_boundary(1, &mut jit, &mut tick_ptr, &mut render_ptr);
        let canceled = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("canceled response");
        let acknowledged = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("cancel acknowledgement");
        assert_eq!(canceled.request_id, 1);
        assert!(!canceled.ok);
        assert_eq!(acknowledged.request_id, 2);
        assert!(workspace.should_run_tick());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn quit_cancels_and_joins_background_preparation() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_canceled = Arc::clone(&canceled);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let (_sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            while !worker_canceled.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            worker_stopped.store(true, Ordering::Release);
        });
        workspace.edit_preparation = Some(EditPreparation {
            request_id: 70,
            canceled,
            receiver,
            worker: Some(worker),
        });
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        client
            .submit(LiveRequest::new(71, LiveCommand::Quit))
            .expect("quit");

        workspace.process_boundary(1, &mut jit, &mut tick_ptr, &mut render_ptr);
        assert!(workspace.should_quit());
        drop(workspace);
        assert!(stopped.load(Ordering::Acquire));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failing_live_edit_tests_restore_source_dispatch_and_state() {
        let (root, config) = project();
        fs::write(
            root.join("tests/main.test.stasis"),
            "import \"../src/main.stasis\";\ntest `tick increments once`(): bool { score = 0; let code: i32 = tick(); return code == 0 && score == 1; }\n",
        )
        .expect("behavioral test");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(4);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let before = fs::read_to_string(root.join("src/main.stasis")).expect("before");
        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                1,
                LiveCommand::Edit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): i32 { score += 4; return 0; }".into()),
                    expected_source_hash: None,
                    preview: false,
                    run_tests: true,
                },
            ),
        );
        assert!(!response.ok);
        assert!(response
            .error
            .expect("error")
            .contains("disk/runtime remained unchanged"));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("old tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(2)));
        assert!(!root.join("build/live-edits").exists());
        fs::remove_dir_all(root).ok();
    }
}
