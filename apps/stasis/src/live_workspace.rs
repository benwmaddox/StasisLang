use serde_json::{json, Value};
use stasis_compiler::backend::jit::{JitEnginePackage, JitProcess, JitScalarValue, JitStateLayout};
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use stasis_compiler::backend::EngineEntrypoints;
use stasis_compiler::compiler::CompileError;
use stasis_compiler::frontend::module_graph::parse_imports;
use stasis_compiler::frontend::workshop::{
    find_workshop_symbols, load_workshop_edit_workspace, load_workshop_project,
    load_workshop_source_workspace, plan_workshop_semantic_edits, workshop_completion_items,
    workshop_direct_import_files, workshop_reachable_files, workshop_source_hash,
    workshop_source_items, write_workshop_semantic_plan, write_workshop_semantic_receipt,
    ExpectedReload, WorkshopCompletionItem, WorkshopSemanticEdit, WorkshopSemanticEditBatch,
    WorkshopSemanticEditOperation, WorkshopSemanticEditPlan, WorkshopSourceFile,
    WorkshopSourceItem, WorkshopSourceItemKind, WorkshopSymbolSelector,
};
use stasis_language_service::{
    DiagnosticSeverity as LanguageDiagnosticSeverity, LanguageCompletionSnapshot,
    LanguageInlayHintKind, LanguageNavigationSnapshot, LanguageService,
    LiveIndexedCollection as LanguageLiveCollection, LiveObservation, LiveObservationBatch,
};
use stasis_runner::live::{
    compare_live_validation_values, CompletionContext, CompletionIndex, CompletionItem,
    CompletionQuery, CompletionScope, LiveCommand, LiveEditOperation, LiveIndexedCollection,
    LiveRequest, LiveResponse, LiveResponseSendError, LiveRuntimeIdentity, LiveSessionServer,
    LiveSymbolTarget, ScratchWorkspace, MAX_LIVE_WATCHES,
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
const MAX_STAGED_TEST_FAILURE_CHARS: usize = 1024;
const MAX_PRIVATE_SYMBOL_HINT_FILES: usize = 64;
const MAX_WATCH_PREDICATE_SCAN_PER_TICK: usize = 4096;
const MAX_INSPECT_VALUES: usize = 4096;
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
    indexed_collections: Vec<IndexedCollectionCompletion>,
}

#[derive(Clone)]
struct IndexedCollectionCompletion {
    path: String,
    fields: Vec<(String, String)>,
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
    indexed_collections: Vec<IndexedCollectionCompletion>,
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
    state_inspection_subscription: Option<(u64, usize, bool)>,
    watch_polling_enabled: bool,
    validation_snapshot: Option<stasis_dynload::JitRuntimeStateSnapshot>,
    host_entry_revision: u64,
    session_id: String,
    language_service: LanguageService,
    language_paths: BTreeSet<String>,
    input_override: Option<Vec<stasis_runner::live::LivePointerInput>>,
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
        let session_id = format!(
            "{}:{}",
            std::process::id(),
            workshop_source_hash(&config.project_root.to_string_lossy())
        );
        let language_service =
            LanguageService::new(config.project_root.to_string_lossy().to_string())?;
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
            indexed_collections: Vec::new(),
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
            state_inspection_subscription: None,
            watch_polling_enabled: true,
            validation_snapshot: None,
            host_entry_revision: stasis_dynload::jit_host_entry_targets()
                .map_or(0, |targets| targets.revision),
            session_id,
            language_service,
            language_paths: BTreeSet::new(),
            input_override: None,
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
        let response = if response.ok && response.runtime_identity.is_none() {
            response.with_runtime_identity(self.runtime_identity())
        } else {
            response
        };
        match self.server.respond(response) {
            Ok(()) => {}
            Err(LiveResponseSendError::Full(response)) => {
                self.pending_responses.push_back(response);
            }
            Err(LiveResponseSendError::Disconnected) => self.quit = true,
        }
    }

    fn runtime_identity(&self) -> LiveRuntimeIdentity {
        let mut identity = LiveRuntimeIdentity {
            session_id: self.session_id.clone(),
            generation: self.host_entry_revision,
            source_hashes: self
                .source_files
                .iter()
                .map(|file| (file.path.clone(), workshop_source_hash(&file.source)))
                .collect(),
            indexed_collections: self
                .indexed_collections
                .iter()
                .map(|collection| LiveIndexedCollection {
                    path: collection.path.clone(),
                    fields: collection.fields.iter().cloned().collect(),
                })
                .collect(),
            complete: true,
        };
        if serde_json::to_vec(&identity).is_ok_and(|encoded| encoded.len() > 32 * 1024) {
            identity.source_hashes.clear();
            identity.indexed_collections.clear();
            identity.complete = false;
        }
        identity
    }

    pub(crate) fn should_run_tick(&self) -> bool {
        !self.paused || self.step_remaining > 0
    }

    pub(crate) fn after_tick(&mut self) {
        if self.paused && self.step_remaining > 0 {
            self.step_remaining -= 1;
        }
        if let Some(pointers) = self.input_override.as_mut() {
            for pointer in pointers {
                pointer.went_down = false;
                pointer.went_up = false;
            }
        }
    }

    pub(crate) fn apply_input_override(
        &self,
        host_i32: &mut [i32],
        host_f32: &mut [f32],
    ) -> Result<(), String> {
        const I_COUNT: usize = 7;
        const I_DROPPED: usize = 8;
        const I_BASE: usize = 544;
        const I_STRIDE: usize = 4;
        const F_STRIDE: usize = 6;
        const F_LOGICAL_W: usize = 50;
        const F_LOGICAL_H: usize = 51;
        let Some(pointers) = self.input_override.as_ref() else {
            return Ok(());
        };
        if host_i32.len() < I_BASE + pointers.len() * I_STRIDE
            || host_f32.len() < pointers.len() * F_STRIDE
            || host_f32.len() <= F_LOGICAL_H
        {
            return Err("host frame buffers are too small for live input".to_string());
        }
        let width = host_f32[F_LOGICAL_W];
        let height = host_f32[F_LOGICAL_H];
        if width <= 0.0 || height <= 0.0 {
            return Err("live input requires a positive logical viewport".to_string());
        }
        host_i32[I_COUNT] = pointers.len() as i32;
        host_i32[I_DROPPED] = 0;
        for (slot, pointer) in pointers.iter().enumerate() {
            if pointer.x < 0
                || pointer.y < 0
                || pointer.x as f32 > width
                || pointer.y as f32 > height
            {
                return Err(format!(
                    "live pointer {} is outside the {}x{} viewport",
                    pointer.id, width, height
                ));
            }
            let ib = I_BASE + slot * I_STRIDE;
            let fb = slot * F_STRIDE;
            host_i32[ib] = pointer.id;
            host_i32[ib + 1] = i32::from(pointer.is_down);
            host_i32[ib + 2] = i32::from(pointer.went_down);
            host_i32[ib + 3] = i32::from(pointer.went_up);
            host_f32[fb] = pointer.x as f32;
            host_f32[fb + 1] = pointer.y as f32;
            host_f32[fb + 2] = 0.0;
            host_f32[fb + 3] = 0.0;
            host_f32[fb + 4] = (pointer.x as f32 / width).clamp(0.0, 1.0);
            host_f32[fb + 5] = (pointer.y as f32 / height).clamp(0.0, 1.0);
        }
        Ok(())
    }

    fn clear_runtime_pointer_input(&self) -> Result<(), String> {
        const I_COUNT: usize = 7;
        const I_DROPPED: usize = 8;
        const I_BASE: usize = 544;
        const I_STRIDE: usize = 4;
        const MAX_POINTERS: usize = 8;
        let host_i32_hash = crate::hash_global_path("host_i32");
        stasis_dynload::fill_registered_global_i32_array(
            host_i32_hash,
            0,
            I_BASE,
            MAX_POINTERS * I_STRIDE,
            0,
        )?;
        stasis_dynload::fill_registered_global_i32_array(
            host_i32_hash,
            0,
            I_COUNT,
            I_DROPPED - I_COUNT + 1,
            0,
        )
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
        let runtime_identity = self.runtime_identity();
        if let Some((every_ticks, limit, concise)) = self.state_inspection_subscription {
            if tick % every_ticks == 0 {
                let response = match inspect_all_scalars(jit, limit, concise) {
                    Ok((kind, data)) => LiveResponse::success(0, tick, kind, data),
                    Err(error) => LiveResponse::failure(0, tick, error),
                }
                .with_runtime_identity(runtime_identity.clone());
                match self.server.respond(response) {
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
        if !self.watch_polling_enabled {
            return;
        }
        if self.dropped_watch_events > 0 {
            let dropped = self.dropped_watch_events;
            match self.server.respond(
                LiveResponse::success(
                    0,
                    tick,
                    "watch_backpressure",
                    json!({"dropped_events": dropped}),
                )
                .with_runtime_identity(runtime_identity.clone()),
            ) {
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
                    match self.server.respond(
                        LiveResponse::success(
                            0,
                            tick,
                            "watch_error",
                            json!({"path": path, "error": error}),
                        )
                        .with_runtime_identity(runtime_identity.clone()),
                    ) {
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
            match self.server.respond(
                LiveResponse::success(
                    0,
                    tick,
                    "watch",
                    json!({"path": path, "value": observed, "inspection": value}),
                )
                .with_runtime_identity(runtime_identity.clone()),
            ) {
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
            LiveCommand::CaptureFrame { artifact } => {
                let artifact = validate_capture_artifact(&artifact)?;
                let directory = self
                    .config
                    .project_root
                    .join(&self.config.output)
                    .join("gauntlet-captures");
                std::fs::create_dir_all(&directory)
                    .map_err(|error| format!("failed creating live capture directory: {error}"))?;
                let path = directory.join(format!("{artifact}.png"));
                if path.exists() {
                    std::fs::remove_file(&path)
                        .map_err(|error| format!("failed replacing prior live capture: {error}"))?;
                }
                stasis_dynload::schedule_runtime_screenshot(&path)?;
                Ok((
                    "capture_scheduled",
                    json!({"artifact": artifact, "path": path, "next_presented_frame": true}),
                ))
            }
            LiveCommand::SetInputState { pointers } => {
                validate_live_pointers(&pointers)?;
                self.input_override = (!pointers.is_empty()).then_some(pointers);
                Ok((
                    "input_state_set",
                    json!({"pointer_count": self.input_override.as_ref().map_or(0, Vec::len)}),
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
                    "references": LanguageNavigationSnapshot::new(self.source_files.clone())
                        .references(&symbol, limit)?,
                }),
            )),
            LiveCommand::Diagnostics => self.language_diagnostics(),
            LiveCommand::Hover { file, offset } => self.language_hover(&file, offset, tick, jit),
            LiveCommand::Definition { file, offset } => self.language_definition(&file, offset),
            LiveCommand::OrganizeImports { file } => self.language_organize_imports(&file),
            LiveCommand::QuickFixes { file } => self.language_quick_fixes(&file),
            LiveCommand::InlayHints { file } => self.language_inlay_hints(&file),
            LiveCommand::CallHierarchy { file, offset } => {
                self.language_call_hierarchy(&file, offset)
            }
            LiveCommand::TypeHierarchy { file, offset } => {
                self.language_type_hierarchy(&file, offset)
            }
            LiveCommand::RenamePreview {
                file,
                offset,
                new_name,
            } => self.rename_preview(&file, offset, &new_name),
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
            LiveCommand::ValidationReinitialize => {
                let main_rc = jit.execute_i32_noarg_by_name("main")?;
                if main_rc != 0 {
                    Err(format!("guest main() returned non-zero status {main_rc}"))
                } else {
                    self.input_override = Some(Vec::new());
                    self.clear_runtime_pointer_input()?;
                    let startup_tick_status = jit.execute_i32_noarg_by_name("tick")?;
                    self.validation_snapshot =
                        Some(stasis_dynload::snapshot_jit_runtime_state_bounded(
                            MAX_STATE_SNAPSHOT_BYTES,
                        )?);
                    Ok((
                        "validation_reinitialized",
                        json!({
                            "main_status": main_rc,
                            "startup_tick_status": startup_tick_status,
                            "captured": true,
                        }),
                    ))
                }
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
            LiveCommand::InspectAll {
                limit,
                concise,
                every_ticks,
            } => match every_ticks {
                Some(0) => {
                    self.state_inspection_subscription = None;
                    self.watch_polling_enabled = false;
                    Ok(("state_inspection_unsubscribed", json!({})))
                }
                Some(ticks) => {
                    self.state_inspection_subscription =
                        Some((ticks.clamp(1, u32::MAX as u64), limit, concise));
                    self.watch_polling_enabled = true;
                    inspect_all_scalars(jit, limit, concise)
                }
                None => inspect_all_scalars(jit, limit, concise),
            },
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

    fn sync_language_service(&mut self) -> Result<(), String> {
        let desired =
            load_workshop_source_workspace(&self.config.project_root, &self.config.entry)?
                .into_iter()
                .map(|file| {
                    (
                        self.config
                            .project_root
                            .join(&file.path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        file.source.clone(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
        let desired_paths = desired.keys().cloned().collect::<BTreeSet<_>>();
        for path in self.language_paths.difference(&desired_paths) {
            self.language_service.remove_disk_document(path);
        }
        let changed = {
            let snapshot = self.language_service.snapshot();
            desired
                .iter()
                .filter(|(path, source)| {
                    snapshot
                        .document(path)
                        .is_none_or(|document| document.text.as_ref() != source.as_str())
                })
                .map(|(path, source)| (path.clone(), source.clone()))
                .collect::<Vec<_>>()
        };
        for (path, source) in changed {
            self.language_service.set_disk_document(path, source);
        }
        self.language_paths = desired_paths;
        Ok(())
    }

    fn language_document_path(&self, file: &str) -> String {
        self.config
            .project_root
            .join(file)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn language_diagnostics(&mut self) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let report = self.language_service.diagnostics();
        Ok((
            "diagnostics",
            json!({
                "revision": report.revision.get(),
                "diagnostics": report.diagnostics.into_iter().map(|diagnostic| json!({
                    "file": diagnostic.path,
                    "start": diagnostic.range.start,
                    "end": diagnostic.range.end,
                    "severity": match diagnostic.severity {
                        LanguageDiagnosticSeverity::Error => "error",
                        LanguageDiagnosticSeverity::Warning => "warning",
                        LanguageDiagnosticSeverity::Information => "information",
                        LanguageDiagnosticSeverity::Hint => "hint",
                    },
                    "code": diagnostic.code,
                    "source": diagnostic.source,
                    "message": diagnostic.message,
                })).collect::<Vec<_>>()
            }),
        ))
    }

    fn language_definition(
        &mut self,
        file: &str,
        offset: usize,
    ) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let locations = self.language_service.definition(&request_path, offset)?;
        Ok((
            "definition",
            json!({
                "file": file,
                "offset": offset,
                "locations": locations.into_iter().map(|location| json!({
                    "file": location.path,
                    "start": location.range.start,
                    "end": location.range.end,
                })).collect::<Vec<_>>()
            }),
        ))
    }

    fn language_hover(
        &mut self,
        file: &str,
        offset: usize,
        tick: u64,
        jit: &JitProcess,
    ) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let Some(static_hover) = self.language_service.hover(&request_path, offset)? else {
            return Ok((
                "hover",
                json!({"file": file, "offset": offset, "hover": null}),
            ));
        };
        if matches!(
            static_hover.kind.as_str(),
            "global" | "field" | "state_path"
        ) {
            if let Ok(inspection) = jit.inspect_state_query(&static_hover.symbol) {
                let identity = self.runtime_identity();
                self.language_service
                    .publish_live_observations(LiveObservationBatch {
                        session_id: identity.session_id,
                        generation: identity.generation,
                        source_hashes: identity.source_hashes,
                        indexed_collections: identity
                            .indexed_collections
                            .into_iter()
                            .map(|collection| LanguageLiveCollection {
                                path: collection.path,
                                fields: collection.fields,
                            })
                            .collect(),
                        complete: identity.complete,
                        observations: vec![LiveObservation {
                            path: static_hover.symbol.clone(),
                            type_name: static_hover.type_name.clone(),
                            value: live_observation_text(&inspection),
                            tick,
                        }],
                    });
            }
        }
        let hover = self
            .language_service
            .hover(&request_path, offset)?
            .unwrap_or(static_hover);
        Ok((
            "hover",
            json!({
                "file": file,
                "offset": offset,
                "hover": {
                    "start": hover.range.start,
                    "end": hover.range.end,
                    "symbol": hover.symbol,
                    "kind": hover.kind,
                    "type_name": hover.type_name,
                    "owner": hover.owner,
                    "signatures": hover.signatures,
                    "documentation": hover.documentation,
                    "live_value": hover.live_value,
                }
            }),
        ))
    }

    fn language_organize_imports(&mut self, file: &str) -> Result<(&'static str, Value), String> {
        self.language_code_actions(file, "source.organizeImports")
    }

    fn language_quick_fixes(&mut self, file: &str) -> Result<(&'static str, Value), String> {
        self.language_code_actions(file, "quickfix")
    }

    fn language_code_actions(
        &mut self,
        file: &str,
        requested_kind: &str,
    ) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let actions = self
            .language_service
            .code_actions(&request_path, &[requested_kind.to_string()])?;
        Ok((
            "code_actions",
            json!({
                "file": file,
                "actions": actions.into_iter().map(|action| json!({
                    "title": action.title,
                    "kind": action.kind,
                    "preferred": action.preferred,
                    "diagnostic_code": action.diagnostic_code,
                    "edits": action.edits.into_iter().map(|edit| json!({
                        "file": edit.path,
                        "start": edit.range.start,
                        "end": edit.range.end,
                        "new_text": edit.new_text,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }),
        ))
    }

    fn language_inlay_hints(&mut self, file: &str) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let hints = self.language_service.inlay_hints(&request_path)?;
        Ok((
            "inlay_hints",
            json!({
                "file": file,
                "hints": hints.into_iter().map(|hint| json!({
                    "position": hint.position,
                    "start": hint.anchor.start,
                    "end": hint.anchor.end,
                    "kind": match hint.kind {
                        LanguageInlayHintKind::Type => "type",
                        LanguageInlayHintKind::Parameter => "parameter",
                    },
                    "label": hint.label,
                })).collect::<Vec<_>>(),
            }),
        ))
    }

    fn language_call_hierarchy(
        &mut self,
        file: &str,
        offset: usize,
    ) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let items = self
            .language_service
            .prepare_call_hierarchy(&request_path, offset)?;
        let mut hierarchy = Vec::new();
        for item in items {
            let incoming = self.language_service.incoming_calls(&item.symbol_id)?;
            let outgoing = self.language_service.outgoing_calls(&item.symbol_id)?;
            hierarchy.push(json!({
                "symbol_id": item.symbol_id,
                "name": item.name,
                "detail": item.detail,
                "file": item.location.path,
                "start": item.location.range.start,
                "end": item.location.range.end,
                "incoming": incoming.into_iter().map(|relation| json!({
                    "symbol_id": relation.item.symbol_id,
                    "name": relation.item.name,
                    "file": relation.item.location.path,
                    "call_ranges": relation.from_ranges.into_iter().map(|location| json!({
                        "file": location.path,
                        "start": location.range.start,
                        "end": location.range.end,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "outgoing": outgoing.into_iter().map(|relation| json!({
                    "symbol_id": relation.item.symbol_id,
                    "name": relation.item.name,
                    "file": relation.item.location.path,
                    "call_ranges": relation.from_ranges.into_iter().map(|location| json!({
                        "file": location.path,
                        "start": location.range.start,
                        "end": location.range.end,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }));
        }
        Ok((
            "call_hierarchy",
            json!({"file": file, "offset": offset, "items": hierarchy}),
        ))
    }

    fn language_type_hierarchy(
        &mut self,
        file: &str,
        offset: usize,
    ) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let items = self
            .language_service
            .prepare_type_hierarchy(&request_path, offset)?;
        let mut hierarchy = Vec::new();
        for item in items {
            let containers = self.language_service.type_supertypes(&item.symbol_id)?;
            let components = self.language_service.type_subtypes(&item.symbol_id)?;
            hierarchy.push(json!({
                "symbol_id": item.symbol_id,
                "name": item.name,
                "detail": item.detail,
                "file": item.location.path,
                "start": item.location.range.start,
                "end": item.location.range.end,
                "containers": containers.into_iter().map(|related| json!({
                    "symbol_id": related.symbol_id,
                    "name": related.name,
                    "file": related.location.path,
                })).collect::<Vec<_>>(),
                "components": components.into_iter().map(|related| json!({
                    "symbol_id": related.symbol_id,
                    "name": related.name,
                    "file": related.location.path,
                })).collect::<Vec<_>>(),
            }));
        }
        Ok((
            "type_hierarchy",
            json!({"file": file, "offset": offset, "items": hierarchy}),
        ))
    }

    fn rename_preview(
        &mut self,
        file: &str,
        offset: usize,
        new_name: &str,
    ) -> Result<(&'static str, Value), String> {
        self.sync_language_service()?;
        let request_path = self.language_document_path(file);
        let plan = self
            .language_service
            .rename(&request_path, offset, new_name)?;
        let edits = plan
            .edits
            .iter()
            .map(|edit| {
                json!({
                    "file": edit.path,
                    "start": edit.range.start,
                    "end": edit.range.end,
                    "new_text": edit.new_text,
                })
            })
            .collect::<Vec<_>>();
        Ok((
            "rename_preview",
            json!({
                "validated": true,
                "revision": plan.revision.get(),
                "old_name": plan.old_name,
                "new_name": plan.new_name,
                "kind": plan.kind,
                "owner": plan.owner,
                "edits": edits,
            }),
        ))
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
        let query_arg = query.filter(|query| !query.is_empty());
        let query = query_arg.unwrap_or("").to_ascii_lowercase();
        let kind = kind.map(parse_kind).transpose()?;
        if files.len() > 16 {
            return Err("symbol search accepts at most 16 starting files".to_string());
        }
        let default_scope = files.is_empty();
        let test_default_scope = default_scope && kind == Some(WorkshopSourceItemKind::Test);
        let mut scope_files = if test_default_scope {
            self.source_items
                .iter()
                .filter(|item| item.kind == WorkshopSourceItemKind::Test)
                .map(|item| normalize_file(&item.file))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        } else if default_scope {
            vec![normalize_file(&self.config.entry.to_string_lossy())]
        } else {
            files.iter().map(|file| normalize_file(file)).collect()
        };
        if default_scope && !test_default_scope {
            let direct_imports =
                workshop_direct_import_files(&self.source_files, &self.config.entry)
                    .unwrap_or_else(|_| {
                        self.best_effort_loaded_direct_imports(&normalize_file(
                            &self.config.entry.to_string_lossy(),
                        ))
                    });
            scope_files.extend(direct_imports);
        }
        let available_files = self
            .source_files
            .iter()
            .map(|file| normalize_file(&file.path))
            .collect::<BTreeSet<_>>();
        for file in &scope_files {
            if !available_files.contains(file) {
                return Err(format!("symbol search file is not in the project: {file}"));
            }
        }
        let scope_files = scope_files.into_iter().collect::<BTreeSet<_>>();
        let matches = |item: &WorkshopSourceItem| {
            item.exposure.is_public()
                && item.kind != WorkshopSourceItemKind::Imports
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
        let returned = items.len();
        let mut result = json!({"items": items});
        if default_scope && !test_default_scope {
            let hint_files = scope_files
                .iter()
                .cloned()
                .chain(
                    scope_files
                        .iter()
                        .flat_map(|file| self.best_effort_loaded_direct_imports(file)),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(MAX_PRIVATE_SYMBOL_HINT_FILES)
                .collect::<Vec<_>>();
            result["_hint_files"] = json!(hint_files);
        }
        if offset.saturating_add(returned) < total {
            let mut args = serde_json::Map::new();
            if !files.is_empty() {
                args.insert("files".into(), json!(files));
            }
            if let Some(query) = query_arg {
                args.insert("query".into(), json!(query));
            }
            if let Some(kind) = kind {
                args.insert("kind".into(), json!(kind));
            }
            if let Some(owner) = owner {
                args.insert("owner".into(), json!(owner));
            }
            args.insert("page".into(), json!(page.saturating_add(1)));
            args.insert("limit".into(), json!(limit));
            result["next"] = json!({"tool": "list_symbols", "args": args});
        }
        Ok(("symbols", result))
    }

    fn best_effort_loaded_direct_imports(&self, file_path: &str) -> Vec<String> {
        let file_path = normalize_file(file_path);
        let available = self
            .source_files
            .iter()
            .map(|file| normalize_file(&file.path))
            .collect::<BTreeSet<_>>();
        let Some(source) = self
            .source_files
            .iter()
            .find(|file| normalize_file(&file.path) == file_path)
            .map(|file| file.source.as_str())
        else {
            return Vec::new();
        };
        workshop_direct_import_files(&self.source_files, Path::new(&file_path)).unwrap_or_else(
            |_| {
                parse_imports(&file_path, source)
                    .map(|imports| {
                        imports
                            .into_iter()
                            .map(|import| normalize_file(&import.target))
                            .filter(|target| available.contains(target))
                            .collect()
                    })
                    .unwrap_or_default()
            },
        )
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
                let mut index = snapshot.index.clone();
                extend_indexed_collection_completion(
                    &mut index,
                    &snapshot.indexed_collections,
                    &buffer,
                    cursor,
                );
                let query = snapshot
                    .language
                    .query_with_index(index, &buffer, cursor, limit, &context);
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
        jit.accept_staged_candidate(prepared.candidate);
        if swap_preview.layout_changed && self.validation_snapshot.is_some() {
            self.validation_snapshot =
                stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES).ok();
        }
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
                .filter(|item| item.exposure.is_public() && !is_static_type_field(item))
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
        self.indexed_collections = jit
            .state_layout()
            .collections
            .into_iter()
            .map(|collection| IndexedCollectionCompletion {
                path: collection.path,
                fields: collection
                    .fields
                    .into_iter()
                    .map(|field| (field.field, field.type_name))
                    .collect(),
            })
            .collect();
        self.completion_snapshot = Arc::new(CompletionSnapshot {
            index: self.completion.clone(),
            language: LanguageCompletionSnapshot::new(
                self.source_items.clone(),
                self.source_files.clone(),
            ),
            indexed_collections: self.indexed_collections.clone(),
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
        let mut index = self.completion.clone();
        extend_indexed_collection_completion(&mut index, &self.indexed_collections, buffer, cursor);
        self.completion_snapshot
            .language
            .query_with_index(index, buffer, cursor, limit, context)
    }
}

fn validate_capture_artifact(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(
            "capture artifact must contain 1..=80 ASCII letters, digits, '-' or '_'".to_string(),
        );
    }
    Ok(value)
}

fn validate_live_pointers(
    pointers: &[stasis_runner::live::LivePointerInput],
) -> Result<(), String> {
    if pointers.len() > 8 {
        return Err("live input supports at most eight pointers".to_string());
    }
    let mut ids = BTreeSet::new();
    for pointer in pointers {
        if pointer.id < 0 || pointer.x < 0 || pointer.y < 0 {
            return Err("live input ids and coordinates must be non-negative".to_string());
        }
        if !ids.insert(pointer.id) {
            return Err(format!("duplicate live pointer id {}", pointer.id));
        }
        if pointer.went_down && (!pointer.is_down || pointer.went_up) {
            return Err("went_down requires is_down and cannot coincide with went_up".to_string());
        }
        if pointer.went_up && pointer.is_down {
            return Err("went_up requires is_down=false".to_string());
        }
    }
    Ok(())
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

fn extend_indexed_collection_completion(
    index: &mut CompletionIndex,
    indexed_collections: &[IndexedCollectionCompletion],
    buffer: &str,
    cursor: usize,
) {
    if let Some((collection_path, receiver)) = indexed_completion_receiver(buffer, cursor) {
        if let Some(collection) = indexed_collections
            .iter()
            .find(|collection| collection.path == collection_path)
        {
            index.extend(
                collection
                    .fields
                    .iter()
                    .map(|(field, type_name)| CompletionItem {
                        text: format!("{receiver}{field}"),
                        kind: "field".to_string(),
                        detail: format!(
                            "{type_name} via indexed state collection {collection_path}"
                        ),
                        type_name: Some(type_name.clone()),
                        source: Some("runtime state layout".to_string()),
                        selector: None,
                        scope: None,
                    }),
            );
        }
    }
}

fn indexed_completion_receiver<'a>(buffer: &'a str, cursor: usize) -> Option<(&'a str, &'a str)> {
    let cursor = cursor.min(buffer.len());
    let prefix = buffer.get(..cursor)?;
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
    if collection_path.split('.').any(|segment| {
        let mut bytes = segment.bytes();
        !bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return None;
    }
    Some((collection_path, receiver))
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
        stage_live_test_assets(&config.project_root, &root, canceled)?;
        let manifest = config.project_root.join("stasis.json");
        if manifest.is_file() {
            fs::copy(&manifest, root.join("stasis.json"))
                .map_err(|error| format!("failed staging stasis.json: {error}"))?;
        }
        let staged_files = staged_test_source_closure(config, files, canceled)?;
        for file in staged_files {
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

pub fn run_project_tests_bounded(project_root: &Path, canceled: &AtomicBool) -> Result<(), String> {
    let executable = locate_stasis_executable()?.ok_or_else(|| {
        "baseline tests require a stasis executable beside the running binary".to_string()
    })?;
    run_staged_test_process(&executable, project_root, canceled)
}

fn staged_test_source_closure(
    config: &LiveRunConfig,
    candidate_files: &[WorkshopSourceFile],
    canceled: &AtomicBool,
) -> Result<Vec<WorkshopSourceFile>, String> {
    let test_files = workshop_source_items(candidate_files)?
        .into_iter()
        .filter(|item| item.kind == WorkshopSourceItemKind::Test)
        .map(|item| normalize_file(&item.file))
        .collect::<BTreeSet<_>>();
    let mut staged = BTreeMap::new();
    for test_file in test_files {
        check_preparation_canceled(canceled)?;
        for imported in load_workshop_project(&config.project_root, Path::new(&test_file))? {
            check_preparation_canceled(canceled)?;
            let path = normalize_file(&imported.path);
            let source_path = Path::new(&path);
            if source_path.extension().and_then(|value| value.to_str()) != Some("stasis") {
                continue;
            }
            if source_path
                .components()
                .next()
                .is_some_and(|component| component.as_os_str() == "build")
            {
                return Err(format!(
                    "staged test import may not read build output: {path}"
                ));
            }
            staged.entry(path.clone()).or_insert(WorkshopSourceFile {
                path,
                source: imported.source,
            });
        }
    }
    for candidate in candidate_files {
        staged.insert(normalize_file(&candidate.path), candidate.clone());
    }
    Ok(staged.into_values().collect())
}

fn stage_live_test_assets(
    project_root: &Path,
    overlay_root: &Path,
    canceled: &AtomicBool,
) -> Result<(), String> {
    check_preparation_canceled(canceled)?;
    let source = project_root.join("assets");
    if !source.exists() {
        return Ok(());
    }
    copy_live_test_asset_directory(&source, &overlay_root.join("assets"), canceled)
}

fn copy_live_test_asset_directory(
    source: &Path,
    destination: &Path,
    canceled: &AtomicBool,
) -> Result<(), String> {
    check_preparation_canceled(canceled)?;
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed creating {}: {error}", destination.display()))?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("failed reading {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed enumerating {}: {error}", source.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        check_preparation_canceled(canceled)?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed inspecting {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_live_test_asset_directory(&entry.path(), &target, canceled)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).map_err(|error| {
                format!(
                    "failed staging live test asset {}: {error}",
                    entry.path().display()
                )
            })?;
            check_preparation_canceled(canceled)?;
        }
    }
    Ok(())
}

fn locate_stasis_executable() -> Result<Option<PathBuf>, String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("failed locating stasis test executable: {error}"))?;
    Ok(locate_stasis_executable_from(&current))
}

fn locate_stasis_executable_from(current: &Path) -> Option<PathBuf> {
    if current.file_stem().and_then(|stem| stem.to_str()) == Some("stasis") {
        return Some(current.to_path_buf());
    }
    let sibling = current.with_file_name(if cfg!(windows) {
        "stasis.exe"
    } else {
        "stasis"
    });
    if sibling.is_file() {
        return Some(sibling);
    }
    let Some(debug_directory) = current.parent().and_then(Path::parent) else {
        return None;
    };
    let candidate = debug_directory.join(if cfg!(windows) {
        "stasis.exe"
    } else {
        "stasis"
    });
    candidate.is_file().then_some(candidate)
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
    Err(format!(
        "staged live tests failed: {}",
        format_staged_test_failure(&stdout, &stderr)
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
        if count >= MAX_STAGED_TEST_DIAGNOSTIC_BYTES {
            captured.clear();
            captured.extend_from_slice(&buffer[count - MAX_STAGED_TEST_DIAGNOSTIC_BYTES..count]);
        } else {
            let overflow = captured
                .len()
                .saturating_add(count)
                .saturating_sub(MAX_STAGED_TEST_DIAGNOSTIC_BYTES);
            if overflow > 0 {
                captured.drain(..overflow);
            }
            captured.extend_from_slice(&buffer[..count]);
        }
    }
    Ok(String::from_utf8_lossy(&captured).into_owned())
}

fn format_staged_test_failure(stdout: &str, stderr: &str) -> String {
    let combined = format!("{stdout}\n{stderr}");
    let mut structured_fallback = None;
    for line in combined.lines().rev() {
        let line = line.trim();
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(message) = record
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            if record.get("code").and_then(Value::as_str) == Some("command_failed") {
                return bounded_test_failure_text(message);
            }
            structured_fallback.get_or_insert_with(|| message.to_string());
        }
    }
    if let Some(message) = structured_fallback {
        return bounded_test_failure_text(&message);
    }
    let fallback = combined.trim();
    if fallback.is_empty() {
        "test process exited without diagnostics".to_string()
    } else {
        bounded_test_failure_text(fallback)
    }
}

fn bounded_test_failure_text(text: &str) -> String {
    let length = text.chars().count();
    if length <= MAX_STAGED_TEST_FAILURE_CHARS {
        return text.to_string();
    }
    const MARKER: &str = "[truncated] ";
    let keep = MAX_STAGED_TEST_FAILURE_CHARS.saturating_sub(MARKER.chars().count());
    let tail = text
        .chars()
        .skip(length.saturating_sub(keep))
        .collect::<String>();
    format!("{MARKER}{tail}")
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
            ":references SYMBOL [--limit N]", ":diagnostics",
            ":hover FILE OFFSET", ":definition FILE OFFSET", ":organize-imports FILE",
            ":quick-fixes FILE",
            ":inlay-hints FILE",
            ":call-hierarchy FILE OFFSET", ":type-hierarchy FILE OFFSET",
            ":rename FILE OFFSET NEW_NAME",
            ":validate PATH OP VALUE [--frames N]",
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
        ":diagnostics",
        ":hover",
        ":definition",
        ":organize-imports",
        ":quick-fixes",
        ":inlay-hints",
        ":call-hierarchy",
        ":type-hierarchy",
        ":rename",
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

fn live_observation_text(inspection: &Value) -> String {
    let observed = inspection_observed_value(inspection);
    let scalar = observed
        .get("value")
        .cloned()
        .unwrap_or_else(|| observed.clone());
    scalar
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| scalar.to_string())
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
    let limit = limit.clamp(1, MAX_INSPECT_VALUES);
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
    let mut remaining_values = limit.saturating_sub(items.len());
    let mut collection_rows_truncated = false;
    let visible_collection_count = layout.collections.len().min(limit);
    let collections = layout
        .collections
        .iter()
        .take(visible_collection_count)
        .enumerate()
        .map(|(collection_index, collection)| -> Result<Value, String> {
            let active_count = memory
                .entries
                .iter()
                .find(|entry| entry.path == collection.path && entry.kind == "collection_field")
                .and_then(|entry| entry.active_count)
                .unwrap_or_else(|| u64::try_from(collection.capacity).unwrap_or(0))
                .min(u64::try_from(collection.capacity).unwrap_or(0));
            let field_value_count = collection.fields.len().max(1);
            let collections_left = visible_collection_count - collection_index;
            let fair_value_share = remaining_values / collections_left.max(1);
            let row_limit = fair_value_share / field_value_count;
            let visible_rows = usize::try_from(active_count)
                .unwrap_or(usize::MAX)
                .min(row_limit);
            let mut row_values = Vec::with_capacity(visible_rows);
            for index in 0..visible_rows {
                let mut values = Vec::with_capacity(collection.fields.len());
                for field in &collection.fields {
                    let value = jit.read_global_collection_scalar(
                        &collection.path,
                        &field.field,
                        i32::try_from(index).map_err(|_| "collection row index exceeds i32")?,
                    )?;
                    values.push(compact_scalar_value(value));
                }
                row_values.push(values);
            }
            remaining_values = remaining_values.saturating_sub(visible_rows * field_value_count);
            let rows_truncated = visible_rows < usize::try_from(active_count).unwrap_or(usize::MAX);
            collection_rows_truncated |= rows_truncated;
            Ok(json!({
                "path": collection.path,
                "kind": "collection",
                "element_shape": collection.element_shape,
                "capacity": collection.capacity,
                "active_count": active_count,
                "fields": collection.fields,
                "row_start": 0,
                "row_values": row_values,
                "rows_truncated": rows_truncated,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let structs = layout.structs.iter().take(limit).collect::<Vec<_>>();
    Ok((
        "state_inspection",
        json!({
            "total": total,
            "limit": limit,
            "truncated": total > limit || layout.collections.len() > limit || layout.structs.len() > limit || collection_rows_truncated,
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

fn compact_scalar_value(value: JitScalarValue) -> Value {
    match value {
        JitScalarValue::I32(value) => json!(value),
        JitScalarValue::F32(value) => json!(value),
        JitScalarValue::F64(value) => json!(value),
        JitScalarValue::Bool(value) => json!(value),
        JitScalarValue::U8(value) => json!(value),
        JitScalarValue::U16(value) => json!(value),
        JitScalarValue::U32(value) => json!(value),
    }
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

    #[test]
    fn gauntlet_capture_ids_and_input_edges_are_bounded() {
        assert!(validate_capture_artifact("candidate-0001").is_ok());
        assert!(validate_capture_artifact("../escape").is_err());
        assert!(
            validate_live_pointers(&[stasis_runner::live::LivePointerInput {
                id: 0,
                x: 480,
                y: 270,
                is_down: true,
                went_down: true,
                went_up: false,
            }])
            .is_ok()
        );
        assert!(
            validate_live_pointers(&[stasis_runner::live::LivePointerInput {
                id: 0,
                x: 0,
                y: 0,
                is_down: false,
                went_down: true,
                went_up: false,
            }])
            .is_err()
        );
    }

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

    #[test]
    fn staged_live_tests_receive_project_assets() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_root = std::env::temp_dir().join(format!("stasis_live_assets_project_{stamp}"));
        let overlay_root = std::env::temp_dir().join(format!("stasis_live_assets_overlay_{stamp}"));
        fs::create_dir_all(project_root.join("assets/generated")).expect("generated assets");
        fs::write(project_root.join("assets/manifest.json"), b"manifest").expect("manifest");
        fs::write(project_root.join("assets/generated/unit.png"), b"png").expect("png");

        stage_live_test_assets(&project_root, &overlay_root, &AtomicBool::new(false))
            .expect("stage assets");

        assert_eq!(
            fs::read(overlay_root.join("assets/manifest.json")).expect("staged manifest"),
            b"manifest"
        );
        assert_eq!(
            fs::read(overlay_root.join("assets/generated/unit.png")).expect("staged png"),
            b"png"
        );
        fs::remove_dir_all(project_root).ok();
        fs::remove_dir_all(overlay_root).ok();
    }

    #[test]
    fn staged_live_test_asset_copy_honors_cancellation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_root =
            std::env::temp_dir().join(format!("stasis_canceled_assets_project_{stamp}"));
        let overlay_root =
            std::env::temp_dir().join(format!("stasis_canceled_assets_overlay_{stamp}"));
        fs::create_dir_all(project_root.join("assets/generated")).expect("generated assets");
        fs::write(project_root.join("assets/generated/unit.png"), b"png").expect("png");

        let error = stage_live_test_assets(&project_root, &overlay_root, &AtomicBool::new(true))
            .expect_err("canceled staging");

        assert!(error.contains("canceled"));
        assert!(!overlay_root.join("assets/generated/unit.png").exists());
        fs::remove_dir_all(project_root).ok();
        fs::remove_dir_all(overlay_root).ok();
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
    fn test_symbol_default_scope_uses_all_known_test_files() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        fs::write(
            root.join("tests/secondary.test.stasis"),
            "test `secondary behavior`(): bool { return 2 == 2; }\n",
        )
        .expect("secondary test");
        let (jit, _) = compile(&config);
        let (_, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");

        let (_, result) = workspace
            .symbols(None, Some("test"), &[], None, 0, 32)
            .expect("test symbols");

        assert_eq!(result.as_object().unwrap().len(), 1);
        assert_eq!(result["items"][0]["kind"], "test");
        assert_eq!(result["items"][1]["kind"], "test");

        let (_, filtered) = workspace
            .symbols(Some("secondary"), Some("test"), &[], None, 0, 1)
            .expect("filtered test symbols");
        assert_eq!(filtered["items"][0]["name"], "secondary behavior");

        workspace
            .source_items
            .retain(|item| item.kind != WorkshopSourceItemKind::Test);
        let (_, empty) = workspace
            .symbols(None, Some("test"), &[], None, 0, 32)
            .expect("empty test symbols");
        assert_eq!(empty["items"], json!([]));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn live_edit_batch_plans_all_symbols_as_one_transaction() {
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
    fn state_inspection_includes_bounded_collection_rows() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "struct Enemy { hp: i32; speed: f32; }\nglobal enemies: Enemy[2];\nfunction main(): i32 { enemies[0].hp = 9; enemies[0].speed = 2.5; return 0; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write source");
        let (jit, _) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (_, data) = inspect_all_scalars(&jit, 8, false).expect("state inspection");
        assert_eq!(data["collections"][0]["path"], "enemies");
        assert_eq!(data["collections"][0]["row_start"], 0);
        assert_eq!(
            data["collections"][0]["row_values"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(data["collections"][0]["row_values"][0][0], 9);
        assert_eq!(data["collections"][0]["row_values"][0][1], 2.5);
        assert!(serde_json::to_vec(&data).expect("encode inspection").len() < 4096);
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
    fn staged_test_failure_prefers_final_structured_command_message() {
        let stdout = format!(
            "{}\n{}\n",
            "balance dump\n".repeat(1_000),
            json!({
                "code": "command_failed",
                "message": "2 test(s) failed: enemy balance remains bounded"
            })
        );
        let total = AtomicUsize::new(0);
        let overflow = AtomicBool::new(false);
        let stdout =
            drain_bounded_test_output(std::io::Cursor::new(stdout.as_bytes()), &total, &overflow)
                .expect("tail capture");
        assert!(!overflow.load(Ordering::Acquire));

        assert_eq!(
            format_staged_test_failure(&stdout, "runner detail"),
            "2 test(s) failed: enemy balance remains bounded"
        );
    }

    #[test]
    fn staged_tests_include_recursive_project_local_test_imports() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        fs::write(
            root.join("stasis.json"),
            r#"{"manifest_version":1,"name":"staged_imports","entry":"src/main.stasis","tests":"tests","output":"build"}"#,
        )
        .expect("manifest");
        fs::create_dir_all(root.join("tools")).expect("tools directory");
        fs::write(
            root.join("tools/nested.stasis"),
            "function helper_base(): bool { return true; }\n",
        )
        .expect("nested helper");
        fs::write(
            root.join("tools/helper.stasis"),
            concat!(
                "import \"nested.stasis\";\n",
                "function helper(): bool { return helper_base(); }\n",
            ),
        )
        .expect("test helper");
        fs::write(
            root.join("tests/main.test.stasis"),
            concat!(
                "import \"../tools/helper.stasis\";\n",
                "test `project helper import is available`(): bool { return false; }\n",
            ),
        )
        .expect("disk test");
        let mut candidate_files =
            load_workshop_edit_workspace(&root, &config.entry).expect("candidate files");
        let candidate_test = candidate_files
            .iter_mut()
            .find(|file| file.path == "tests/main.test.stasis")
            .expect("candidate test");
        candidate_test.source = concat!(
            "import \"../tools/helper.stasis\";\n",
            "test `project helper import is available`(): bool { return helper(); }\n",
        )
        .to_string();

        let closure =
            staged_test_source_closure(&config, &candidate_files, &AtomicBool::new(false))
                .expect("staged source closure");
        assert!(closure
            .iter()
            .any(|file| file.path == "tools/helper.stasis"));
        assert!(closure
            .iter()
            .any(|file| file.path == "tools/nested.stasis"));
        assert!(closure
            .iter()
            .find(|file| file.path == "tests/main.test.stasis")
            .is_some_and(|file| file.source.contains("return helper()")));

        run_staged_tests(&config, &candidate_files, 424_242, &AtomicBool::new(false))
            .expect("staged tests with project-local helper");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn staged_test_failure_fallback_is_bounded_and_marked() {
        let failure = format_staged_test_failure(&"x".repeat(2_000), "");
        assert_eq!(failure.chars().count(), MAX_STAGED_TEST_FAILURE_CHARS);
        assert!(failure.starts_with("[truncated] "));
        assert!(failure.ends_with(&"x".repeat(128)));
    }

    #[test]
    fn staged_test_output_drain_retains_tail_and_flags_overflow() {
        let marker = b"FINAL_TAIL";
        let mut bytes = vec![b'x'; MAX_STAGED_TEST_OUTPUT_BYTES + 1 - marker.len()];
        bytes.extend_from_slice(marker);
        let total = AtomicUsize::new(0);
        let overflow = AtomicBool::new(false);
        let captured = drain_bounded_test_output(std::io::Cursor::new(bytes), &total, &overflow)
            .expect("drain output");
        assert_eq!(captured.len(), MAX_STAGED_TEST_DIAGNOSTIC_BYTES);
        assert!(captured.ends_with("FINAL_TAIL"));
        assert_eq!(
            total.load(Ordering::Acquire),
            MAX_STAGED_TEST_OUTPUT_BYTES + 1
        );
        assert!(overflow.load(Ordering::Acquire));
    }

    #[test]
    fn renamed_release_host_finds_sibling_stasis_executable() {
        let root = std::env::temp_dir().join(format!(
            "stasis_executable_lookup_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        fs::create_dir_all(&root).expect("lookup root");
        let executable_name = if cfg!(windows) {
            "stasis.exe"
        } else {
            "stasis"
        };
        let sibling = root.join(executable_name);
        fs::write(&sibling, b"test executable").expect("sibling executable");
        let renamed = root.join(if cfg!(windows) {
            "stasis-gauntlet-fixed.exe"
        } else {
            "stasis-gauntlet-fixed"
        });

        assert_eq!(locate_stasis_executable_from(&renamed), Some(sibling));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn live_runtime_candidate_excludes_test_only_symbols() {
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
    fn rename_preview_is_compiler_validated_and_does_not_write_sources() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        let source_path = root.join("src/main.stasis");
        let before = fs::read_to_string(&source_path).expect("source before preview");
        let offset = before.find("score +=").expect("score use") + 2;
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
                40,
                LiveCommand::RenamePreview {
                    file: "src/main.stasis".into(),
                    offset,
                    new_name: "points".into(),
                },
            ),
        );

        assert!(response.ok, "rename response: {response:?}");
        let identity = response
            .runtime_identity
            .as_ref()
            .expect("accepted runtime identity");
        assert!(identity.source_hashes.contains_key("src/main.stasis"));
        assert_eq!(identity.generation, workspace.host_entry_revision);
        let data = response.data.expect("rename preview data");
        assert_eq!(data["validated"], true);
        assert_eq!(data["old_name"], "score");
        assert_eq!(data["new_name"], "points");
        assert!(data["edits"]
            .as_array()
            .is_some_and(|edits| edits.len() == 3));
        assert_eq!(
            fs::read_to_string(&source_path).expect("source after preview"),
            before
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tui_quick_fix_preview_uses_structured_language_service_actions() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        let source_path = root.join("src/main.stasis");
        let before = fs::read_to_string(&source_path).expect("source before quick fix");
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let broken = format!("import \"missing.stasis\";\n{before}");
        fs::write(&source_path, &broken).expect("source with missing import");

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                401,
                LiveCommand::QuickFixes {
                    file: "src/main.stasis".into(),
                },
            ),
        );

        assert!(response.ok, "quick-fix response: {response:?}");
        let actions = response.data.expect("quick-fix data")["actions"]
            .as_array()
            .expect("quick-fix actions")
            .clone();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["kind"], "quickfix");
        assert_eq!(actions[0]["diagnostic_code"], "stasis.missingModule");
        assert_eq!(actions[0]["edits"][0]["new_text"], "");
        assert_eq!(
            fs::read_to_string(&source_path).expect("source after quick-fix preview"),
            broken
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tui_language_queries_share_persistent_service_and_live_hover() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        let source_path = root.join("src/main.stasis");
        let source = fs::read_to_string(&source_path)
            .expect("source")
            .replace(
                "function main(): i32 { score = 1; return 0; }",
                "struct Position { x: i32; }\nstruct Enemy { position: Position; }\nfunction add(amount: i32, bonus: i32): i32 { return amount + bonus; }\nfunction main(): i32 { let initial = add(1, 2); score = initial; return 0; }",
            );
        fs::write(&source_path, &source).expect("source with inferred local");
        let offset = source.find("score +=").expect("score use") + 2;
        let (mut jit, package) = compile(&config);
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;

        let diagnostics = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(41, LiveCommand::Diagnostics),
        );
        assert!(diagnostics.ok);
        assert_eq!(
            diagnostics.data.expect("diagnostics")["diagnostics"],
            json!([])
        );

        let hover = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                42,
                LiveCommand::Hover {
                    file: "src/main.stasis".into(),
                    offset,
                },
            ),
        );
        assert!(hover.ok, "hover response: {hover:?}");
        let hover = hover.data.expect("hover data")["hover"].clone();
        assert_eq!(hover["symbol"], "score");
        assert_eq!(hover["type_name"], "i32");
        assert!(hover["live_value"]
            .as_str()
            .is_some_and(|value| value.contains("tick")));

        let definition = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                43,
                LiveCommand::Definition {
                    file: "src/main.stasis".into(),
                    offset,
                },
            ),
        );
        assert!(definition.ok, "definition response: {definition:?}");
        let locations = definition.data.expect("definition data")["locations"]
            .as_array()
            .expect("locations")
            .clone();
        assert_eq!(locations.len(), 1);
        let start = locations[0]["start"].as_u64().expect("start") as usize;
        let end = locations[0]["end"].as_u64().expect("end") as usize;
        assert_eq!(&source[start..end], "score");

        let inlays = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                44,
                LiveCommand::InlayHints {
                    file: "src/main.stasis".into(),
                },
            ),
        );
        assert!(inlays.ok, "inlay response: {inlays:?}");
        let hints = inlays.data.expect("inlay data")["hints"]
            .as_array()
            .expect("hints")
            .clone();
        assert!(hints
            .iter()
            .any(|hint| hint["kind"] == "type" && hint["label"] == ": i32"));
        assert!(hints
            .iter()
            .any(|hint| { hint["kind"] == "parameter" && hint["label"] == "amount:" }));

        let add_offset = source.find("add(amount").expect("add") + 1;
        let calls = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                45,
                LiveCommand::CallHierarchy {
                    file: "src/main.stasis".into(),
                    offset: add_offset,
                },
            ),
        );
        assert!(calls.ok, "call hierarchy response: {calls:?}");
        let calls = calls.data.expect("call hierarchy");
        assert_eq!(calls["items"][0]["incoming"][0]["name"], "main");
        assert_eq!(calls["items"][0]["outgoing"], json!([]));

        let enemy_offset = source.find("Enemy").expect("Enemy") + 1;
        let types = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                46,
                LiveCommand::TypeHierarchy {
                    file: "src/main.stasis".into(),
                    offset: enemy_offset,
                },
            ),
        );
        assert!(types.ok, "type hierarchy response: {types:?}");
        assert_eq!(
            types.data.expect("type hierarchy")["items"][0]["components"][0]["name"],
            "Position"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn symbol_search_is_filtered_compact_and_hash_free() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "import \"helper.stasis\";\nimport \"game/game.stasis\";\nglobal score: i32;\nfunction main(): i32 { score = 1; return 0; }\nfunction tick(): i32 { score += 1; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("source with import");
        fs::write(
            root.join("src/helper.stasis"),
            "function direct_import_value(): i32 { return 1; }\n",
        )
        .expect("helper source");
        fs::create_dir_all(root.join("src/game/systems")).expect("systems source directory");
        fs::write(
            root.join("src/game/game.stasis"),
            "import \"systems/enemies.stasis\";\nfunction game_value(): i32 { return 1; }\n",
        )
        .expect("game module source");
        fs::write(
            root.join("src/game/systems/enemies.stasis"),
            "function update_enemy_movement(): void { return; }\n",
        )
        .expect("enemy system source");
        let (jit, _package) = compile(&config);
        let (_client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");

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
        assert!(all.get("files").is_none());
        assert!(all.get("imports").is_none());
        assert!(items
            .iter()
            .any(|item| item["name"] == "direct_import_value"));
        assert_eq!(
            all["_hint_files"],
            json!([
                "src/game/game.stasis",
                "src/game/systems/enemies.stasis",
                "src/helper.stasis",
                "src/main.stasis"
            ])
        );

        let game = workspace
            .source_files
            .iter_mut()
            .find(|file| file.path == "src/game/game.stasis")
            .expect("game source");
        game.source = format!("import \"../vendor/unloaded.stasis\";\n{}", game.source);
        let (_, tolerant) = workspace
            .symbols(None, None, &[], None, 0, 32)
            .expect("missing import edges do not block discovery");
        assert!(tolerant["_hint_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file == "src/helper.stasis"));
        assert!(tolerant["_hint_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file == "src/game/systems/enemies.stasis"));

        let (_, paged) = workspace
            .symbols(
                None,
                Some("function"),
                &["src/main.stasis".to_string()],
                None,
                0,
                1,
            )
            .expect("paged symbols");
        assert_eq!(
            paged["next"],
            json!({
                "tool":"list_symbols",
                "args":{
                    "files":["src/main.stasis"],
                    "kind":"function",
                    "page":1,
                    "limit":1
                }
            })
        );
        assert!(paged.get("total").is_none());
        assert!(paged.get("page").is_none());
        assert!(paged.get("limit").is_none());
        let paged_bytes = serde_json::to_vec(&paged).unwrap().len();
        eprintln!("representative compact list_symbols page bytes: {paged_bytes}");
        assert!(paged_bytes < 1024);

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
        assert_eq!(tests["items"].as_array().unwrap().len(), 1);

        let test_file = workspace
            .source_files
            .iter_mut()
            .find(|file| file.path == "tests/main.test.stasis")
            .expect("test source");
        test_file.source = "import \"../vendor/unloaded.stasis\";\n".to_string();
        let (_, unaffected) = workspace
            .symbols(None, Some("test"), &[], None, 0, 32)
            .expect("test discovery does not resolve unsolicited imports");
        assert_eq!(unaffected["items"].as_array().unwrap().len(), 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn validation_snapshot_restores_the_same_runtime_baseline() {
        let _global_guard = crate::jit_test_support::lock();
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
    fn validation_reinitialize_runs_current_main_and_startup_tick_before_snapshot() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "global score: i32;\nglobal host_i32: i32[768];\nfunction main(): i32 { score = 1; return 0; }\nfunction tick(): i32 { score += 1 + host_i32[7]; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("input-sensitive source");
        let mut host_i32 = vec![0; 768];
        host_i32[7] = 1;
        host_i32[544] = 7;
        host_i32[545] = 1;
        host_i32[546] = 1;
        stasis_dynload::register_global_i32_array(
            crate::hash_global_path("host_i32"),
            0,
            host_i32.as_mut_ptr(),
            host_i32.len(),
        );
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;

        jit.write_global_scalar("score", JitScalarValue::I32(9))
            .expect("mutate score");
        let reinitialized = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(44, LiveCommand::ValidationReinitialize),
        );
        assert!(reinitialized.ok, "{:?}", reinitialized.error);
        assert_eq!(reinitialized.kind, "validation_reinitialized");
        let reinitialized_data = reinitialized.data.expect("reinitialize data");
        assert_eq!(reinitialized_data["main_status"], 0);
        assert_eq!(reinitialized_data["startup_tick_status"], 0);
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(2)));
        assert_eq!(host_i32[7], 0);
        assert!(host_i32[544..576].iter().all(|value| *value == 0));

        jit.write_global_scalar("score", JitScalarValue::I32(7))
            .expect("mutate score again");
        let restored = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(45, LiveCommand::ValidationRestore),
        );
        assert!(restored.ok, "{:?}", restored.error);
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(2)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn human_runtime_validation_restores_live_state_after_frames() {
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
    fn hidden_live_view_stops_snapshot_and_watch_polling() {
        let _global_guard = crate::jit_test_support::lock();
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
                90,
                LiveCommand::InspectAll {
                    limit: 32,
                    concise: false,
                    every_ticks: Some(30),
                },
            ),
        );
        assert_eq!(response.kind, "state_inspection");

        workspace.publish_watches(29, &jit);
        assert!(client
            .receive_timeout(std::time::Duration::from_millis(10))
            .is_err());
        workspace.publish_watches(30, &jit);
        let refresh = client
            .receive_timeout(std::time::Duration::from_millis(10))
            .expect("tick-based state refresh");
        assert_eq!(refresh.request_id, 0);
        assert_eq!(refresh.tick, 30);
        assert_eq!(refresh.kind, "state_inspection");

        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    91,
                    LiveCommand::Watch {
                        path: "10 / score".into()
                    },
                ),
            )
            .ok
        );

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                92,
                LiveCommand::InspectAll {
                    limit: 32,
                    concise: false,
                    every_ticks: Some(0),
                },
            ),
        );
        assert_eq!(response.kind, "state_inspection_unsubscribed");
        assert!(
            run_request(
                &client,
                &mut workspace,
                &mut jit,
                &mut tick_ptr,
                &mut render_ptr,
                LiveRequest::new(
                    93,
                    LiveCommand::Set {
                        path: "score".into(),
                        expression: "0".into(),
                        preview: false,
                    },
                ),
            )
            .ok
        );
        workspace.publish_watches(60, &jit);
        assert!(client
            .receive_timeout(std::time::Duration::from_millis(10))
            .is_err());

        let response = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                94,
                LiveCommand::InspectAll {
                    limit: 32,
                    concise: false,
                    every_ticks: Some(30),
                },
            ),
        );
        assert_eq!(response.kind, "state_inspection");
        workspace.publish_watches(61, &jit);
        let watch = client
            .receive_timeout(std::time::Duration::from_secs(1))
            .expect("remembered watch resumes with the view");
        assert_eq!(watch.kind, "watch_error");
        assert_eq!(watch.data.expect("watch data")["path"], "10 / score");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn expression_watch_reports_and_deduplicates_evaluation_errors() {
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
    fn live_batch_can_add_and_call_a_helper_after_hot_swap() {
        let _global_guard = crate::jit_test_support::lock();
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
                1,
                LiveCommand::EditBatch {
                    edits: vec![
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Update,
                            target: LiveSymbolTarget {
                                name: "tick".into(),
                                kind: Some("function".into()),
                                file: Some("src/main.stasis".into()),
                                owner: None,
                                signature: None,
                            },
                            source: Some(
                                "function tick(): i32 { score = new_helper(); return 0; }".into(),
                            ),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget {
                                name: "new_helper".into(),
                                kind: Some("function".into()),
                                file: Some("src/main.stasis".into()),
                                owner: None,
                                signature: Some("new_helper(): i32".into()),
                            },
                            source: Some("function new_helper(): i32 { return 9; }".into()),
                            expected_source_hash: None,
                        },
                    ],
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.kind, "edit_applied");

        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("hot-swapped tick");
        assert_eq!(jit.read_global_scalar("score"), Ok(JitScalarValue::I32(9)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn layout_hot_swap_keeps_validation_restore_and_new_helper_calls_safe() {
        let _global_guard = crate::jit_test_support::lock();
        let (root, config) = project();
        fs::write(
            root.join("src/main.stasis"),
            "struct Game { ticks: i32; swaps: i32; }\nglobal game: Game;\nfunction main(): i32 { game.ticks = 0; game.swaps = 0; return 0; }\nfunction tick(): i32 { game.ticks += 1; return 0; }\nfunction render(): i32 { return 0; }\nfunction on_code_swap(): void { game.swaps += 1; return; }\n",
        )
        .expect("layout source");
        let (mut jit, package) = compile(&config);
        jit.execute_i32_noarg_by_name("main").expect("main");
        let (client, server) = stasis_runner::live::live_session(8);
        let mut workspace = LiveWorkspace::new(server, config, &jit).expect("workspace");
        let mut tick_ptr = package.tick_code_ptr;
        let mut render_ptr = package.render_code_ptr;
        let snapshot = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(1, LiveCommand::ValidationSnapshot),
        );
        assert!(snapshot.ok, "{:?}", snapshot.error);

        let preview = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(
                2,
                LiveCommand::EditBatch {
                    edits: vec![
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Update,
                            target: LiveSymbolTarget {
                                name: "Game".into(),
                                kind: Some("struct".into()),
                                file: Some("src/main.stasis".into()),
                                owner: Some("Game".into()),
                                signature: None,
                            },
                            source: Some(
                                "struct Game { ticks: i32; swaps: i32; phase: i32; }".into(),
                            ),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Update,
                            target: LiveSymbolTarget {
                                name: "tick".into(),
                                kind: Some("function".into()),
                                file: Some("src/main.stasis".into()),
                                owner: None,
                                signature: None,
                            },
                            source: Some(
                                "function tick(): i32 { game.ticks += phase_value(); return 0; }"
                                    .into(),
                            ),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget {
                                name: "phase_value".into(),
                                kind: Some("function".into()),
                                file: Some("src/main.stasis".into()),
                                owner: None,
                                signature: Some("phase_value(): i32".into()),
                            },
                            source: Some("function phase_value(): i32 { return 2; }".into()),
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
                            source: Some(
                                "function render(): i32 { if (game.phase == phase_zero()) { game.phase = phase_one(); } if (game.phase == phase_two()) { game.phase = phase_three(); } if (game.phase == phase_four()) { game.phase = 0; } return 0; }"
                                    .into(),
                            ),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget { name: "phase_zero".into(), kind: Some("function".into()), file: Some("src/main.stasis".into()), owner: None, signature: Some("phase_zero(): i32".into()) },
                            source: Some("function phase_zero(): i32 { return 0; }".into()),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget { name: "phase_one".into(), kind: Some("function".into()), file: Some("src/main.stasis".into()), owner: None, signature: Some("phase_one(): i32".into()) },
                            source: Some("function phase_one(): i32 { return 1; }".into()),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget { name: "phase_two".into(), kind: Some("function".into()), file: Some("src/main.stasis".into()), owner: None, signature: Some("phase_two(): i32".into()) },
                            source: Some("function phase_two(): i32 { return 2; }".into()),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget { name: "phase_three".into(), kind: Some("function".into()), file: Some("src/main.stasis".into()), owner: None, signature: Some("phase_three(): i32".into()) },
                            source: Some("function phase_three(): i32 { return 3; }".into()),
                            expected_source_hash: None,
                        },
                        stasis_runner::live::LiveEdit {
                            operation: LiveEditOperation::Add,
                            target: LiveSymbolTarget { name: "phase_four".into(), kind: Some("function".into()), file: Some("src/main.stasis".into()), owner: None, signature: Some("phase_four(): i32".into()) },
                            source: Some("function phase_four(): i32 { return 4; }".into()),
                            expected_source_hash: None,
                        },
                    ],
                    preview: false,
                    run_tests: false,
                },
            ),
        );
        assert!(preview.ok, "{:?}", preview.error);
        assert_eq!(preview.kind, "edit_preview");
        let applied = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(3, LiveCommand::Apply { run_tests: false }),
        );
        assert!(applied.ok, "{:?}", applied.error);
        let restored = run_request(
            &client,
            &mut workspace,
            &mut jit,
            &mut tick_ptr,
            &mut render_ptr,
            LiveRequest::new(4, LiveCommand::ValidationRestore),
        );
        assert!(restored.ok, "{:?}", restored.error);

        stasis_dynload::invoke_noarg_i32(tick_ptr as usize).expect("hot-swapped tick");
        assert_eq!(
            jit.read_global_scalar("game.ticks"),
            Ok(JitScalarValue::I32(2))
        );
        for (name, expected) in [
            ("phase_zero", 0),
            ("phase_one", 1),
            ("phase_two", 2),
            ("phase_three", 3),
            ("phase_four", 4),
        ] {
            assert_eq!(jit.execute_i32_noarg_by_name(name), Ok(expected), "{name}");
        }
        stasis_dynload::invoke_noarg_i32(render_ptr as usize).expect("hot-swapped render");
        assert_eq!(
            jit.read_global_scalar("game.phase"),
            Ok(JitScalarValue::I32(1))
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn layout_edit_previews_then_preserves_state_and_initializes_new_field() {
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let indexed_buffer = "game.enemies[0].";
        let indexed_query = workspace.completion_query(
            indexed_buffer,
            indexed_buffer.len(),
            10,
            &CompletionContext::default(),
        );
        assert!(
            indexed_query
                .items
                .iter()
                .any(|item| item.text == "game.enemies[0].hp"),
            "indexed collection completion should use runtime layout fields: {:?}",
            indexed_query.items
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
        let _global_guard = crate::jit_test_support::lock();
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
