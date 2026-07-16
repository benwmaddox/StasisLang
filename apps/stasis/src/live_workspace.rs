use serde_json::{json, Value};
use stasis_compiler::backend::jit::{JitEnginePackage, JitProcess, JitScalarValue};
use stasis_compiler::backend::EngineEntrypoints;
use stasis_compiler::compiler::CompileError;
use stasis_compiler::frontend::workshop::{
    find_workshop_symbols, load_workshop_edit_workspace, plan_workshop_semantic_edits,
    workshop_source_hash, workshop_source_items, write_workshop_semantic_plan,
    write_workshop_semantic_receipt, ExpectedReload, WorkshopSemanticEdit,
    WorkshopSemanticEditBatch, WorkshopSemanticEditOperation, WorkshopSemanticEditPlan,
    WorkshopSourceFile, WorkshopSourceItem, WorkshopSourceItemKind, WorkshopSymbolSelector,
};
use stasis_runner::live::{
    CompletionIndex, CompletionItem, LiveCommand, LiveEditOperation, LiveRequest, LiveResponse,
    LiveResponseSendError, LiveSessionServer, LiveSymbolTarget, ScratchWorkspace,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};

const REQUESTS_PER_TICK: usize = 8;
const MAX_PENDING_LIVE_REQUESTS: usize = 64;
const MAX_LIVE_EDIT_SOURCE_BYTES: usize = 256 * 1024;
const MAX_LIVE_STATE_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_LIVE_TRANSACTION_ASSIGNMENTS: usize = 64;
const MAX_STAGED_TEST_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_STAGED_TEST_DIAGNOSTIC_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub struct LiveRunConfig {
    pub project_root: PathBuf,
    pub entry: PathBuf,
    pub output: PathBuf,
}

impl LiveRunConfig {
    pub fn new(project_root: PathBuf, entry: PathBuf, output: PathBuf) -> Self {
        Self {
            project_root,
            entry,
            output,
        }
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    plan: WorkshopSemanticEditPlan,
    receipt: PathBuf,
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
    restore: bool,
    action: PreparedAction,
    tests_ran: bool,
    candidate: JitProcess,
    package: JitEnginePackage,
    source_items: Vec<WorkshopSourceItem>,
    input_hashes: BTreeMap<String, String>,
}

struct EditPreparation {
    request_id: u64,
    canceled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<PreparedEdit, String>>,
    worker: Option<std::thread::JoinHandle<()>>,
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
    Plan {
        plan: WorkshopSemanticEditPlan,
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
    scratch: ScratchWorkspace,
    watches: BTreeMap<String, Option<JitScalarValue>>,
    pending_plan: Option<WorkshopSemanticEditPlan>,
    pending_requests: VecDeque<LiveRequest>,
    pending_responses: VecDeque<LiveResponse>,
    self_write_hashes: BTreeMap<PathBuf, String>,
    edit_preparation: Option<EditPreparation>,
    dropped_watch_events: u64,
}

impl Drop for LiveWorkspace {
    fn drop(&mut self) {
        if let Some(mut preparation) = self.edit_preparation.take() {
            preparation.canceled.store(true, Ordering::Release);
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
            scratch: ScratchWorkspace::default(),
            watches: BTreeMap::new(),
            pending_plan: None,
            pending_requests: VecDeque::new(),
            pending_responses: VecDeque::new(),
            self_write_hashes: BTreeMap::new(),
            edit_preparation: None,
            dropped_watch_events: 0,
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
        let _ = self.refresh_completion(jit);
    }

    pub(crate) fn consumes_self_write(&mut self, path: &Path) -> bool {
        let Some(expected_hash) = self.self_write_hashes.get(path) else {
            return false;
        };
        let matches = std::fs::read_to_string(path)
            .is_ok_and(|source| workshop_source_hash(&source) == *expected_hash);
        if !matches {
            self.self_write_hashes.remove(path);
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
        for path in paths {
            let Ok(value) = jit.read_global_scalar(&path) else {
                continue;
            };
            let prior = self.watches.get(&path).copied().flatten();
            if prior == Some(value) {
                continue;
            }
            self.watches.insert(path.clone(), Some(value));
            match self.server.respond(LiveResponse::success(
                0,
                tick,
                "watch",
                json!({"path": path, "value": value}),
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
            LiveCommand::Symbols { query, page, limit } => {
                self.symbols(query.as_deref(), page, limit)
            }
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
            LiveCommand::Complete {
                buffer,
                cursor,
                limit,
            } => Ok((
                "completion",
                json!({"items": self.completion.complete(&buffer, cursor, limit)}),
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
            ),
            LiveCommand::Preview => self
                .pending_plan
                .as_ref()
                .map(|plan| ("edit_preview", json!({"validated": true, "plan": plan})))
                .ok_or_else(|| "no validated live semantic preview is pending".to_string()),
            LiveCommand::Apply { run_tests } => {
                let plan = self
                    .pending_plan
                    .clone()
                    .ok_or_else(|| "no validated live semantic preview is pending".to_string())?;
                self.start_edit_preparation(
                    request_id,
                    EditPreparationInput::Plan {
                        plan,
                        restore: false,
                        run_tests,
                        action: PreparedAction::ApplyPending,
                    },
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
                        restore: true,
                        run_tests,
                        action: PreparedAction::Undo { index },
                    },
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
                            restore: false,
                            run_tests,
                            action: PreparedAction::Redo { index },
                        },
                    )
                }
            }
            LiveCommand::Inspect { path } => inspect_scalar(jit, &path),
            LiveCommand::Watch { path } => {
                let value = jit.read_global_scalar(&path)?;
                self.watches.insert(path.clone(), Some(value));
                Ok((
                    "watch_added",
                    json!({"path": path, "value": value, "tick": tick}),
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
        page: u32,
        limit: usize,
    ) -> Result<(&'static str, Value), String> {
        let query = query.unwrap_or("").to_ascii_lowercase();
        let matching = self
            .source_items
            .iter()
            .filter(|item| {
                query.is_empty()
                    || item.name.to_ascii_lowercase().contains(&query)
                    || item.signature.to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect::<Vec<_>>();
        let limit = limit.clamp(1, 200);
        let offset = usize::try_from(page)
            .unwrap_or(usize::MAX)
            .saturating_mul(limit);
        let total = matching.len();
        let items = matching
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|item| {
                json!({
                    "kind": item.kind,
                    "name": item.name,
                    "owner": item.owner,
                    "file": item.file,
                    "signature": item.signature,
                    "source_hash": item.source_hash,
                })
            })
            .collect::<Vec<_>>();
        Ok((
            "symbols",
            json!({"schema_version": 1, "page": page, "limit": limit, "total": total, "items": items}),
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
    ) -> Result<(&'static str, Value), String> {
        if let Some(preparation) = self.edit_preparation.as_ref() {
            return Err(format!(
                "live edit request {} is still preparing; cancel or wait for it",
                preparation.request_id
            ));
        }
        validate_edit_input_size(&input)?;
        let config = self.config.clone();
        let canceled = Arc::new(AtomicBool::new(false));
        let worker_canceled = Arc::clone(&canceled);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name(format!("stasis-live-edit-{request_id}"))
            .spawn(move || {
                let result = prepare_edit(request_id, &config, input, &worker_canceled);
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
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                return Some(LiveResponse::failure(preparation.request_id, tick, error));
            }
        };
        if matches!(prepared.action, PreparedAction::Preview) {
            self.pending_plan = Some(prepared.plan.clone());
            return Some(LiveResponse::success(
                prepared.request_id,
                tick,
                "edit_preview",
                json!({"validated": true, "plan": prepared.plan}),
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
        let snapshot =
            stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_LIVE_STATE_SNAPSHOT_BYTES)?;
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
                self.rollback_prepared(&prepared, jit, &snapshot)?;
                return Err(format!(
                    "live receipt failed; disk/runtime remained unchanged: {error}"
                ));
            }
        };

        if let Err(error) = prepared.candidate.activate_staged_runtime() {
            self.rollback_prepared(&prepared, jit, &snapshot)?;
            cleanup_new_receipt(&self.config, &receipt, receipt_existed)?;
            return Err(format!(
                "live runtime activation failed; disk/code/state remained unchanged: {error}"
            ));
        }
        stasis_dynload::restore_jit_runtime_state(&snapshot);
        if let Some(hook) = prepared.package.on_code_swap_code_ptr {
            if let Err(error) = stasis_dynload::invoke_noarg_void(hook as usize) {
                self.rollback_prepared(&prepared, jit, &snapshot)?;
                cleanup_new_receipt(&self.config, &receipt, receipt_existed)?;
                return Err(format!(
                    "on_code_swap failed; disk/code/state remained on the prior version: {error}"
                ));
            }
        }
        *tick_code_ptr = prepared.package.tick_code_ptr;
        *render_code_ptr = prepared.package.render_code_ptr;
        let plan = prepared.plan.clone();
        let action = prepared.action.clone();
        let source_items = prepared.source_items;
        let tests = if prepared.tests_ran {
            "passed"
        } else {
            "skipped"
        };
        *jit = prepared.candidate;
        self.source_items = source_items;
        self.remember_plan_hashes(&plan, prepared.restore);
        let (kind, data) = match action {
            PreparedAction::ApplyNew => {
                self.history.truncate(self.history_cursor);
                self.history.push(HistoryEntry {
                    plan: plan.clone(),
                    receipt: receipt.clone(),
                });
                self.history_cursor = self.history.len();
                (
                    "edit_applied",
                    json!({"plan": plan, "receipt": receipt, "tests": tests}),
                )
            }
            PreparedAction::ApplyPending => {
                self.history.truncate(self.history_cursor);
                self.history.push(HistoryEntry {
                    plan: plan.clone(),
                    receipt: receipt.clone(),
                });
                self.history_cursor = self.history.len();
                self.pending_plan = None;
                (
                    "edit_applied",
                    json!({"plan": plan, "receipt": receipt, "tests": tests}),
                )
            }
            PreparedAction::Undo { index } => {
                self.history_cursor = index;
                (
                    "edit_undone",
                    json!({"index": index, "receipt": receipt, "tests": tests}),
                )
            }
            PreparedAction::Redo { index } => {
                self.history_cursor = index + 1;
                (
                    "edit_redone",
                    json!({"index": index, "receipt": receipt, "tests": tests}),
                )
            }
            PreparedAction::Preview => unreachable!("preview never commits"),
        };
        self.rebuild_completion(jit);
        Ok((kind, data))
    }

    fn rollback_prepared(
        &mut self,
        prepared: &PreparedEdit,
        active: &JitProcess,
        snapshot: &stasis_dynload::JitRuntimeStateSnapshot,
    ) -> Result<(), String> {
        let result = rollback_prepared_disk(&self.config, prepared, active, snapshot);
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
            self.self_write_hashes.insert(
                self.config.project_root.join(&change.file),
                workshop_source_hash(source),
            );
        }
    }

    fn refresh_completion(&mut self, jit: &JitProcess) -> Result<(), String> {
        let files = self.load_files()?;
        self.source_items = workshop_source_items(&files)?;
        self.rebuild_completion(jit);
        Ok(())
    }

    fn rebuild_completion(&mut self, jit: &JitProcess) {
        let mut items = live_command_completions();
        items.extend(self.source_items.iter().map(|item| CompletionItem {
            text: item.name.clone(),
            kind: format!("{:?}", item.kind).to_ascii_lowercase(),
            detail: item.signature.clone(),
        }));
        items.extend(
            jit.global_scalar_paths()
                .into_iter()
                .map(|(path, type_name)| CompletionItem {
                    text: path,
                    kind: "state_path".to_string(),
                    detail: type_name.to_string(),
                }),
        );
        items.extend(self.scratch.list().into_iter().map(|cell| CompletionItem {
            text: cell.name,
            kind: "scratch_cell".to_string(),
            detail: "session-only scratch cell".to_string(),
        }));
        self.completion.replace(items);
    }
}

fn validate_edit_input_size(input: &EditPreparationInput) -> Result<(), String> {
    let source = match input {
        EditPreparationInput::Edit { source, .. } => source.as_deref(),
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
    canceled: &AtomicBool,
) -> Result<PreparedEdit, String> {
    check_preparation_canceled(canceled)?;
    let files = load_workshop_edit_workspace(&config.project_root, &config.entry)?;
    let input_hashes = files
        .iter()
        .map(|file| (file.path.clone(), workshop_source_hash(&file.source)))
        .collect::<BTreeMap<_, _>>();
    let (candidate_files, plan, restore, run_tests, action) = match input {
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
            )
        }
        EditPreparationInput::Plan {
            plan,
            restore,
            run_tests,
            action,
        } => {
            let candidate_files = files_for_plan(&files, &plan, restore)?;
            (candidate_files, plan, restore, run_tests, action)
        }
    };
    if plan.reload.expected_reload == ExpectedReload::ResetRequired {
        return Err(format!(
            "live edit rejected until layout migration support lands in Maddox #153: {}",
            plan.reload.reason
        ));
    }
    check_preparation_canceled(canceled)?;
    let (candidate, package) = compile_candidate(config, &candidate_files)?;
    check_preparation_canceled(canceled)?;
    if run_tests {
        run_staged_tests(config, &candidate_files, request_id, canceled).map_err(|error| {
            format!("live edit tests failed; disk/runtime remained unchanged: {error}")
        })?;
    }
    check_preparation_canceled(canceled)?;
    let source_items = workshop_source_items(&candidate_files)?;
    Ok(PreparedEdit {
        request_id,
        plan,
        restore,
        action,
        tests_ran: run_tests,
        candidate,
        package,
        source_items,
        input_hashes,
    })
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

fn compile_candidate(
    config: &LiveRunConfig,
    files: &[WorkshopSourceFile],
) -> Result<(JitProcess, JitEnginePackage), String> {
    let mut candidate = JitProcess::new();
    candidate.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
        "on_code_swap".to_string(),
    ]);
    for file in files {
        let path = config.project_root.join(&file.path);
        candidate.upsert_file(path.to_string_lossy().to_string(), file.source.clone());
    }
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

fn rollback_prepared_disk(
    config: &LiveRunConfig,
    prepared: &PreparedEdit,
    active: &JitProcess,
    snapshot: &stasis_dynload::JitRuntimeStateSnapshot,
) -> Result<(), String> {
    let rollback =
        write_workshop_semantic_plan(&config.project_root, &prepared.plan, !prepared.restore);
    let activation = active.activate_staged_runtime();
    stasis_dynload::restore_jit_runtime_state(snapshot);
    rollback.map_err(|error| format!("disk rollback failed: {error}"))?;
    activation.map_err(|error| format!("runtime rollback failed: {error}"))
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
            ":symbols [query] [--page N --limit N]",
            ":read NAME [KIND] [--file FILE --owner OWNER --signature SIGNATURE]", ":complete BUFFER",
            ":add KIND NAME FILE ... :end", ":update KIND NAME [FILE] ... :end",
            ":delete KIND NAME [FILE]", ":preview", ":apply", ":changes", ":undo", ":redo",
            ":inspect PATH", ":watch PATH", ":unwatch [PATH]", ":set PATH VALUE",
            ":print VALUE_OR_PATH", ":do ... :end", ":cell put|run|list|clear"
        ],
        "multiline_terminator": ":end",
        "multiline_cancel": ":abort or Ctrl-C",
        "line_editor": "session history and compiler-backed Tab completion",
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
        ":complete",
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
    })
    .collect()
}

fn selector(target: &LiveSymbolTarget) -> Result<WorkshopSymbolSelector, String> {
    Ok(WorkshopSymbolSelector {
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
    let value = jit.read_global_scalar(path)?;
    Ok((
        "inspection",
        json!({"path": path, "static_type": value.type_name(), "value": value}),
    ))
}

fn print_scalar(jit: &JitProcess, expression: &str) -> Result<(&'static str, Value), String> {
    if jit.has_global_path(expression) {
        return inspect_scalar(jit, expression);
    }
    if let Ok(value) = expression.parse::<i32>() {
        return Ok(("print", json!({"static_type": "i32", "value": value})));
    }
    if matches!(expression, "true" | "false") {
        return Ok((
            "print",
            json!({"static_type": "bool", "value": expression == "true"}),
        ));
    }
    Err("live print currently accepts a compiler-indexed scalar path, i32 literal, or bool literal; unsupported expressions fail instead of using a second evaluator".to_string())
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
                if response.request_id == request_id && response.kind != "edit_preparing" {
                    return response;
                }
            }
        }
        panic!("live request {request_id} did not finish");
    }

    fn prepared_tick_edit(config: &LiveRunConfig, request_id: u64) -> PreparedEdit {
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
    fn pause_step_and_watch_events_are_boundary_exact() {
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
                        path: "score".into()
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
        assert_eq!(watch.data.expect("watch data")["value"]["value"], 9);
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
    fn layout_edit_is_rejected_before_disk_or_runtime_change() {
        let (root, config) = project();
        let (mut jit, package) = compile(&config);
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
        assert!(!response.ok);
        assert!(response.error.expect("error").contains("#153"));
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("after"),
            before
        );
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
                    source: Some("function helper(): i32 { return 7; }".into()),
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
                    source: Some("function helper(): i32 { return 8; }".into()),
                    expected_source_hash: Some("stale".into()),
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(!stale.ok);
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("return 7"));

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
        assert!(workspace.completion.complete(":read hel", 9, 10).is_empty());
        assert!(!fs::read_to_string(root.join("src/main.stasis"))
            .expect("source")
            .contains("function helper"));
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
