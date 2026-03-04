use crate::swap::contracts::{
    CompileRequest, CompileResult, CompileStatus, Diagnostic, DiagnosticSeverity, FileChangeEvent,
    FnId, FunctionPatchSet, LayoutHash, RequestId, SwapCommitRequest, SwapCommitResult, TargetMode,
    CONTRACT_VERSION,
};
use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

enum CompilerThreadMessage {
    Compile(CompileRequest),
    Shutdown,
}

pub trait CompilerBackend: Send + 'static {
    fn compile(&mut self, request: CompileRequest) -> CompileResult;
}

impl<F> CompilerBackend for F
where
    F: FnMut(CompileRequest) -> CompileResult + Send + 'static,
{
    fn compile(&mut self, request: CompileRequest) -> CompileResult {
        self(request)
    }
}

/// Dev-mode pipeline with explicit thread/channel boundaries:
/// watcher -> coordinator -> compiler thread -> main-thread safe-point commit gate.
pub struct DevHotSwapPipeline {
    file_change_tx: Sender<FileChangeEvent>,
    file_change_rx: Receiver<FileChangeEvent>,
    compiler_tx: Sender<CompilerThreadMessage>,
    compile_result_rx: Receiver<CompileResult>,
    commit_request_tx: Sender<SwapCommitRequest>,
    commit_request_rx: Receiver<SwapCommitRequest>,
    commit_result_tx: Sender<SwapCommitResult>,
    commit_result_rx: Receiver<SwapCommitResult>,
    pending_files: BTreeSet<PathBuf>,
    target_mode: TargetMode,
    host_set_id: Option<String>,
    host_set_hash: Option<[u8; 32]>,
    in_flight_compile: Option<RequestId>,
    in_flight_compile_started_at: Option<Instant>,
    in_flight_commit: Option<RequestId>,
    in_flight_commit_started_at: Option<Instant>,
    next_request_id: u64,
    last_compile_result: Option<CompileResult>,
    last_compile_duration: Option<Duration>,
    last_commit_result: Option<SwapCommitResult>,
    last_commit_duration: Option<Duration>,
    compiler_thread: Option<JoinHandle<()>>,
}

impl DevHotSwapPipeline {
    pub fn new<B: CompilerBackend>(backend: B) -> Self {
        Self::with_target_mode(backend, TargetMode::JitDev)
    }

    pub fn with_target_mode<B: CompilerBackend>(mut backend: B, target_mode: TargetMode) -> Self {
        let (file_change_tx, file_change_rx) = unbounded::<FileChangeEvent>();
        let (compiler_tx, compiler_rx) = unbounded::<CompilerThreadMessage>();
        let (compile_result_tx, compile_result_rx) = unbounded::<CompileResult>();
        let (commit_request_tx, commit_request_rx) = unbounded::<SwapCommitRequest>();
        let (commit_result_tx, commit_result_rx) = unbounded::<SwapCommitResult>();

        let compiler_thread = thread::spawn(move || {
            while let Ok(message) = compiler_rx.recv() {
                match message {
                    CompilerThreadMessage::Compile(request) => {
                        let result = backend.compile(request);
                        if compile_result_tx.send(result).is_err() {
                            break;
                        }
                    }
                    CompilerThreadMessage::Shutdown => break,
                }
            }
        });

        Self {
            file_change_tx,
            file_change_rx,
            compiler_tx,
            compile_result_rx,
            commit_request_tx,
            commit_request_rx,
            commit_result_tx,
            commit_result_rx,
            pending_files: BTreeSet::new(),
            target_mode,
            host_set_id: None,
            host_set_hash: None,
            in_flight_compile: None,
            in_flight_compile_started_at: None,
            in_flight_commit: None,
            in_flight_commit_started_at: None,
            next_request_id: 1,
            last_compile_result: None,
            last_compile_duration: None,
            last_commit_result: None,
            last_commit_duration: None,
            compiler_thread: Some(compiler_thread),
        }
    }

    pub fn watcher_sender(&self) -> Sender<FileChangeEvent> {
        self.file_change_tx.clone()
    }

    pub fn set_host_set_contract(
        &mut self,
        host_set_id: Option<String>,
        host_set_hash: Option<[u8; 32]>,
    ) {
        self.host_set_id = host_set_id;
        self.host_set_hash = host_set_hash;
    }

    pub fn submit_file_change(&self, event: FileChangeEvent) {
        // If watcher side is disconnected, runtime should continue without panic.
        let _ = self.file_change_tx.send(event);
    }

    /// Runs coordinator work on the caller thread.
    /// Should be called in the runtime loop outside hot gameplay paths.
    pub fn pump_coordinator(&mut self) {
        self.drain_file_changes();
        self.maybe_dispatch_compile_request();
        self.drain_compile_results();
        self.drain_commit_results();
    }

    /// Safe-point gate called between ticks on the main thread.
    pub fn process_commits_at_safe_point<F>(&mut self, mut apply_commit: F) -> usize
    where
        F: FnMut(SwapCommitRequest) -> SwapCommitResult,
    {
        let mut processed = 0usize;
        loop {
            match self.commit_request_rx.try_recv() {
                Ok(request) => {
                    let result = apply_commit(request);
                    let _ = self.commit_result_tx.send(result);
                    processed += 1;
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        processed
    }

    pub fn has_in_flight_work(&self) -> bool {
        self.in_flight_compile.is_some()
            || self.in_flight_commit.is_some()
            || !self.pending_files.is_empty()
    }

    pub fn pending_commit_requests(&self) -> usize {
        self.commit_request_rx.len()
    }

    pub fn last_compile_result(&self) -> Option<&CompileResult> {
        self.last_compile_result.as_ref()
    }

    pub fn last_commit_result(&self) -> Option<&SwapCommitResult> {
        self.last_commit_result.as_ref()
    }

    pub fn last_compile_duration(&self) -> Option<Duration> {
        self.last_compile_duration
    }

    pub fn last_commit_duration(&self) -> Option<Duration> {
        self.last_commit_duration
    }

    fn drain_file_changes(&mut self) {
        loop {
            match self.file_change_rx.try_recv() {
                Ok(change) => {
                    self.pending_files.insert(change.path);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn maybe_dispatch_compile_request(&mut self) {
        if self.in_flight_compile.is_some()
            || self.in_flight_commit.is_some()
            || self.pending_files.is_empty()
        {
            return;
        }

        let request_id = RequestId(self.next_request_id);
        self.next_request_id += 1;

        let changed_files: Vec<PathBuf> = self.pending_files.iter().cloned().collect();
        self.pending_files.clear();

        let mut request = CompileRequest::new(request_id, changed_files, self.target_mode);
        request.host_set_id = self.host_set_id.clone();
        request.host_set_hash = self.host_set_hash;
        let _ = self
            .compiler_tx
            .send(CompilerThreadMessage::Compile(request));
        self.in_flight_compile = Some(request_id);
        self.in_flight_compile_started_at = Some(Instant::now());
    }

    fn drain_compile_results(&mut self) {
        loop {
            match self.compile_result_rx.try_recv() {
                Ok(result) => {
                    let result = if result.contract_version == CONTRACT_VERSION {
                        result
                    } else {
                        CompileResult::failed(
                            result.request_id,
                            vec![Diagnostic {
                                severity: DiagnosticSeverity::Error,
                                message: format!(
                                    "compile contract version mismatch: expected {}, got {}",
                                    CONTRACT_VERSION, result.contract_version
                                ),
                                path: None,
                                line: None,
                                column: None,
                            }],
                        )
                    };
                    self.last_compile_result = Some(result.clone());
                    if self.in_flight_compile == Some(result.request_id) {
                        self.in_flight_compile = None;
                        self.last_compile_duration = self
                            .in_flight_compile_started_at
                            .take()
                            .map(|started| started.elapsed());
                        if result.status == CompileStatus::Success {
                            self.enqueue_commit_request(
                                result.request_id,
                                result.layout_hash,
                                result.fn_patch_set,
                                result.hook_symbol.clone(),
                                result.host_set_id.clone(),
                                result.host_set_hash,
                                result.hook_fn_id,
                                result.state_map.clone(),
                            );
                        }
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn enqueue_commit_request(
        &mut self,
        request_id: RequestId,
        layout_hash: Option<LayoutHash>,
        fn_patch_set: Option<FunctionPatchSet>,
        hook_symbol: Option<String>,
        host_set_id: Option<String>,
        host_set_hash: Option<[u8; 32]>,
        hook_fn_id: Option<FnId>,
        state_map: Option<Vec<crate::swap::contracts::StateMapEntry>>,
    ) {
        let Some(layout_hash) = layout_hash else {
            return;
        };
        let Some(fn_patch_set) = fn_patch_set else {
            return;
        };
        if fn_patch_set.functions.is_empty() {
            return;
        }

        let request = SwapCommitRequest {
            contract_version: CONTRACT_VERSION,
            request_id,
            layout_hash,
            fn_patch_set,
            hook_symbol,
            host_set_id,
            host_set_hash,
            hook_fn_id,
            state_map,
        };
        let _ = self.commit_request_tx.send(request);
        self.in_flight_commit = Some(request_id);
        self.in_flight_commit_started_at = Some(Instant::now());
    }

    fn drain_commit_results(&mut self) {
        loop {
            match self.commit_result_rx.try_recv() {
                Ok(result) => {
                    let result = if result.contract_version == CONTRACT_VERSION {
                        result
                    } else {
                        SwapCommitResult::failed(
                            result.request_id,
                            format!(
                                "commit contract version mismatch: expected {}, got {}",
                                CONTRACT_VERSION, result.contract_version
                            ),
                        )
                    };
                    self.last_commit_result = Some(result.clone());
                    if self.in_flight_commit == Some(result.request_id) {
                        self.in_flight_commit = None;
                        self.last_commit_duration = self
                            .in_flight_commit_started_at
                            .take()
                            .map(|started| started.elapsed());
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}

impl Drop for DevHotSwapPipeline {
    fn drop(&mut self) {
        let _ = self.compiler_tx.send(CompilerThreadMessage::Shutdown);
        if let Some(join_handle) = self.compiler_thread.take() {
            let _ = join_handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swap::contracts::{
        CodeGeneration, Diagnostic, DiagnosticSeverity, FileChangeKind, FnId, FunctionPatch,
        SwapCommitResult, SwapCommitStatus, TextSource,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn eventually(mut condition: impl FnMut() -> bool) {
        for _ in 0..2000 {
            if condition() {
                return;
            }
            thread::yield_now();
            thread::sleep(Duration::from_micros(250));
        }
        panic!("condition not met before timeout");
    }

    fn sample_change(path: &str, revision: u64) -> FileChangeEvent {
        FileChangeEvent::new(
            PathBuf::from(path),
            revision,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        )
    }

    fn sample_patch_set() -> FunctionPatchSet {
        FunctionPatchSet {
            functions: vec![
                FunctionPatch { fn_id: FnId(7) },
                FunctionPatch { fn_id: FnId(11) },
            ],
        }
    }

    #[test]
    fn routes_file_changes_to_compile_then_waits_for_safe_point_commit() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            CompileResult::success(request.request_id, LayoutHash([9; 32]), sample_patch_set())
        });

        pipeline.submit_file_change(sample_change("samples/a.stasis", 1));
        pipeline.pump_coordinator();

        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.pending_commit_requests() == 1
        });

        let compile_result = pipeline
            .last_compile_result()
            .expect("compile result should exist");
        assert_eq!(compile_result.status, CompileStatus::Success);
        assert_eq!(pipeline.last_commit_result(), None);

        let processed = pipeline.process_commits_at_safe_point(|request| {
            assert_eq!(request.hook_symbol, None);
            let swapped = request
                .fn_patch_set
                .functions
                .iter()
                .map(|f| f.fn_id)
                .collect();
            SwapCommitResult::success(request.request_id, swapped, CodeGeneration(2))
        });
        assert_eq!(processed, 1);

        pipeline.pump_coordinator();
        let commit_result = pipeline
            .last_commit_result()
            .expect("commit result should exist");
        assert_eq!(commit_result.status, SwapCommitStatus::Success);
        assert!(!pipeline.has_in_flight_work());
    }

    #[test]
    fn compile_failure_does_not_queue_commit_request() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            CompileResult::failed(
                request.request_id,
                vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: "parse error".to_string(),
                    path: Some(PathBuf::from("samples/bad.stasis")),
                    line: Some(3),
                    column: Some(14),
                }],
            )
        });

        pipeline.submit_file_change(sample_change("samples/bad.stasis", 2));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.last_compile_result().is_some()
        });

        let compile_result = pipeline
            .last_compile_result()
            .expect("compile result should exist");
        assert_eq!(compile_result.status, CompileStatus::Failed);
        assert_eq!(pipeline.pending_commit_requests(), 0);
        assert!(pipeline.last_commit_result().is_none());
        assert!(!pipeline.has_in_flight_work());
    }

    #[test]
    fn coalesces_changes_and_dispatches_single_compile_request() {
        let requests: Arc<Mutex<Vec<CompileRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let mut pipeline = DevHotSwapPipeline::new(move |request: CompileRequest| {
            captured.lock().expect("poisoned").push(request.clone());
            CompileResult::success(request.request_id, LayoutHash([4; 32]), sample_patch_set())
        });

        pipeline.submit_file_change(sample_change("samples/zeta_10.stasis", 1));
        pipeline.submit_file_change(sample_change("samples/zeta_2.stasis", 2));
        pipeline.submit_file_change(sample_change("samples/zeta_2.stasis", 3));
        pipeline.pump_coordinator();

        eventually(|| {
            pipeline.pump_coordinator();
            requests.lock().expect("poisoned").len() == 1
        });

        let captured_requests = requests.lock().expect("poisoned");
        let request = captured_requests.first().expect("request should exist");
        assert_eq!(request.changed_files.len(), 2);
        assert_eq!(request.target_mode, TargetMode::JitDev);
        assert_eq!(
            request.changed_files[0],
            PathBuf::from("samples/zeta_10.stasis")
        );
        assert_eq!(
            request.changed_files[1],
            PathBuf::from("samples/zeta_2.stasis")
        );
    }

    #[test]
    fn with_target_mode_dispatches_aot_compile_request() {
        let requests: Arc<Mutex<Vec<CompileRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let mut pipeline = DevHotSwapPipeline::with_target_mode(
            move |request: CompileRequest| {
                captured.lock().expect("poisoned").push(request.clone());
                CompileResult::success(request.request_id, LayoutHash([5; 32]), sample_patch_set())
            },
            TargetMode::AotProd,
        );

        pipeline.submit_file_change(sample_change("samples/prod.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            requests.lock().expect("poisoned").len() == 1
        });

        let captured_requests = requests.lock().expect("poisoned");
        let request = captured_requests.first().expect("request should exist");
        assert_eq!(request.target_mode, TargetMode::AotProd);
    }

    #[test]
    fn host_set_contract_flows_from_pipeline_to_compile_and_commit_requests() {
        let host_set_id = "editor-host".to_string();
        let host_set_hash = [7u8; 32];
        let mut pipeline = DevHotSwapPipeline::new(move |request: CompileRequest| {
            assert_eq!(request.host_set_id.as_deref(), Some("editor-host"));
            assert_eq!(request.host_set_hash, Some(host_set_hash));
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([6; 32]),
                sample_patch_set(),
                request.host_set_id.clone(),
                request.host_set_hash,
                None,
                None,
                None,
                None,
                None,
                None,
            )
        });
        pipeline.set_host_set_contract(Some(host_set_id), Some(host_set_hash));

        pipeline.submit_file_change(sample_change("samples/host_set.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.pending_commit_requests() == 1
        });

        let processed = pipeline.process_commits_at_safe_point(|request| {
            assert_eq!(request.host_set_id.as_deref(), Some("editor-host"));
            assert_eq!(request.host_set_hash, Some(host_set_hash));
            SwapCommitResult::success(request.request_id, vec![FnId(7)], CodeGeneration(1))
        });
        assert_eq!(processed, 1);
    }

    #[test]
    fn compile_hook_symbol_propagates_to_commit_request() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            CompileResult::success_with_hook_symbol(
                request.request_id,
                LayoutHash([6; 32]),
                sample_patch_set(),
                Some("on_code_swap".to_string()),
            )
        });

        pipeline.submit_file_change(sample_change("samples/hook_symbol.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.pending_commit_requests() == 1
        });

        let processed = pipeline.process_commits_at_safe_point(|request| {
            assert_eq!(request.hook_symbol.as_deref(), Some("on_code_swap"));
            SwapCommitResult::success(request.request_id, vec![FnId(7)], CodeGeneration(1))
        });
        assert_eq!(processed, 1);
    }

    #[test]
    fn compile_hook_fn_id_propagates_to_commit_request() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([6; 32]),
                sample_patch_set(),
                None,
                None,
                Some("on_code_swap".to_string()),
                Some(FnId(55)),
                None,
                None,
                None,
                None,
            )
        });

        pipeline.submit_file_change(sample_change("samples/hook_fn_id.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.pending_commit_requests() == 1
        });

        let processed = pipeline.process_commits_at_safe_point(|request| {
            assert_eq!(request.hook_symbol.as_deref(), Some("on_code_swap"));
            assert_eq!(request.hook_fn_id, Some(FnId(55)));
            SwapCommitResult::success(request.request_id, vec![FnId(7)], CodeGeneration(1))
        });
        assert_eq!(processed, 1);
    }

    #[test]
    fn compile_contract_version_mismatch_is_reported_and_commit_not_queued() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            let mut result = CompileResult::success_with_hook_symbol(
                request.request_id,
                LayoutHash([1; 32]),
                sample_patch_set(),
                Some("on_code_swap".to_string()),
            );
            result.contract_version = CONTRACT_VERSION + 1;
            result
        });

        pipeline.submit_file_change(sample_change("samples/version_mismatch.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.last_compile_result().is_some()
        });

        let compile_result = pipeline
            .last_compile_result()
            .expect("compile result should exist");
        assert_eq!(compile_result.status, CompileStatus::Failed);
        assert_eq!(pipeline.pending_commit_requests(), 0);
        assert!(compile_result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("contract version mismatch")));
    }

    #[test]
    fn commit_contract_version_mismatch_is_reported_as_failed_commit() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            CompileResult::success(request.request_id, LayoutHash([2; 32]), sample_patch_set())
        });

        pipeline.submit_file_change(sample_change("samples/commit_version_mismatch.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.pending_commit_requests() == 1
        });

        pipeline.process_commits_at_safe_point(|request| SwapCommitResult {
            contract_version: CONTRACT_VERSION + 1,
            request_id: request.request_id,
            status: SwapCommitStatus::Success,
            swapped_fn_ids: request
                .fn_patch_set
                .functions
                .iter()
                .map(|f| f.fn_id)
                .collect(),
            new_generation: Some(CodeGeneration(3)),
            error: None,
        });
        pipeline.pump_coordinator();

        let commit_result = pipeline
            .last_commit_result()
            .expect("commit result should exist");
        assert_eq!(commit_result.status, SwapCommitStatus::Failed);
        assert!(commit_result
            .error
            .as_deref()
            .is_some_and(|msg| msg.contains("contract version mismatch")));
        assert!(!pipeline.has_in_flight_work());
    }

    #[test]
    fn success_with_empty_patch_set_does_not_queue_commit() {
        let mut pipeline = DevHotSwapPipeline::new(|request: CompileRequest| {
            CompileResult::success(
                request.request_id,
                LayoutHash([8; 32]),
                FunctionPatchSet {
                    functions: Vec::new(),
                },
            )
        });

        pipeline.submit_file_change(sample_change("samples/noop.stasis", 1));
        eventually(|| {
            pipeline.pump_coordinator();
            pipeline.last_compile_result().is_some()
        });

        let compile_result = pipeline
            .last_compile_result()
            .expect("compile result should exist");
        assert_eq!(compile_result.status, CompileStatus::Success);
        assert_eq!(pipeline.pending_commit_requests(), 0);
        assert!(pipeline.last_commit_result().is_none());
        assert!(!pipeline.has_in_flight_work());
    }
}
