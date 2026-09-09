use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub const LIVE_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_LIVE_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_LIVE_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_LIVE_REQUEST_BYTES: usize = 512 * 1024;
pub const MAX_LIVE_MULTILINE_BYTES: usize = 256 * 1024;
pub const MAX_SCRATCH_CELLS: usize = 64;
pub const MAX_LIVE_WATCHES: usize = 64;
const MAX_LIVE_OUTSTANDING_REQUESTS_PER_CLIENT: usize = 128;
const MAX_LIVE_OUTSTANDING_REQUESTS_PER_SESSION: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveRequest {
    #[serde(default = "live_schema_version")]
    pub schema_version: u16,
    pub request_id: u64,
    #[serde(flatten)]
    pub command: LiveCommand,
}

impl LiveRequest {
    pub fn new(request_id: u64, command: LiveCommand) -> Self {
        Self {
            schema_version: LIVE_SCHEMA_VERSION,
            request_id,
            command,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != LIVE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported live-session schema version {}; expected {}",
                self.schema_version, LIVE_SCHEMA_VERSION
            ));
        }
        if self.request_id == 0 {
            return Err("request_id must be greater than zero".to_string());
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("failed serializing live request: {error}"))?
            .len();
        if bytes > MAX_LIVE_REQUEST_BYTES {
            return Err(format!(
                "live request requires {bytes} bytes; limit is {MAX_LIVE_REQUEST_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

const fn live_schema_version() -> u16 {
    LIVE_SCHEMA_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveCommand {
    Help,
    Status,
    Pause,
    Resume,
    Step {
        #[serde(default = "one_tick")]
        ticks: u32,
    },
    CaptureFrame {
        artifact: String,
    },
    SetInputState {
        #[serde(default)]
        pointers: Vec<LivePointerInput>,
    },
    Cancel {
        request_id: u64,
    },
    Quit,
    Symbols {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        page: u32,
        #[serde(default = "default_symbol_page_limit")]
        limit: usize,
    },
    Read {
        name: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        signature: Option<String>,
    },
    References {
        symbol: String,
        #[serde(default = "default_reference_limit")]
        limit: usize,
    },
    Diagnostics,
    Hover {
        file: String,
        offset: usize,
    },
    Definition {
        file: String,
        offset: usize,
    },
    OrganizeImports {
        file: String,
    },
    QuickFixes {
        file: String,
    },
    InlayHints {
        file: String,
    },
    CallHierarchy {
        file: String,
        offset: usize,
    },
    TypeHierarchy {
        file: String,
        offset: usize,
    },
    RenamePreview {
        file: String,
        offset: usize,
        new_name: String,
    },
    Validate {
        requirement: LiveValidationRequirement,
        #[serde(default)]
        frames: u32,
    },
    ValidationSnapshot,
    ValidationReinitialize,
    ValidationRestore,
    ValidationClear,
    Complete {
        buffer: String,
        cursor: usize,
        #[serde(default = "default_completion_limit")]
        limit: usize,
        #[serde(default)]
        context: CompletionContext,
    },
    Palette {
        #[serde(default)]
        query: String,
        #[serde(default)]
        page: u32,
        #[serde(default = "default_completion_limit")]
        limit: usize,
        #[serde(default)]
        context: CompletionContext,
    },
    Edit {
        operation: LiveEditOperation,
        target: LiveSymbolTarget,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        expected_source_hash: Option<String>,
        #[serde(default)]
        preview: bool,
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    EditBatch {
        edits: Vec<LiveEdit>,
        #[serde(default)]
        preview: bool,
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Preview,
    Apply {
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Changes,
    Undo {
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Redo {
        #[serde(default = "default_true")]
        run_tests: bool,
    },
    Inspect {
        path: String,
    },
    InspectAll {
        #[serde(default = "default_inspect_limit")]
        limit: usize,
        #[serde(default)]
        concise: bool,
        #[serde(default)]
        every_ticks: Option<u64>,
    },
    Watch {
        path: String,
    },
    Unwatch {
        #[serde(default)]
        path: Option<String>,
    },
    Set {
        path: String,
        expression: String,
        #[serde(default)]
        preview: bool,
    },
    Print {
        expression: String,
    },
    Evaluate {
        expression: String,
    },
    Do {
        code: String,
        #[serde(default)]
        preview: bool,
    },
    CellPut {
        name: String,
        code: String,
    },
    CellRun {
        name: String,
        #[serde(default)]
        preview: bool,
    },
    CellList,
    CellClear {
        #[serde(default)]
        name: Option<String>,
    },
    CellPersist {
        name: String,
        target: LiveSymbolTarget,
        #[serde(default)]
        preview: bool,
        #[serde(default = "default_true")]
        run_tests: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivePointerInput {
    pub id: i32,
    pub x: i32,
    pub y: i32,
    #[serde(default)]
    pub is_down: bool,
    #[serde(default)]
    pub went_down: bool,
    #[serde(default)]
    pub went_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveValidationRequirement {
    pub path: String,
    #[serde(default = "default_validation_operator")]
    pub op: String,
    pub value: serde_json::Value,
}

pub fn compare_live_validation_values(
    actual: &serde_json::Value,
    operator: &str,
    expected: &serde_json::Value,
) -> Result<bool, String> {
    if matches!(operator, "eq" | "ne") {
        let equal = actual == expected
            || actual
                .as_f64()
                .zip(expected.as_f64())
                .is_some_and(|(actual, expected)| actual == expected);
        return Ok(if operator == "eq" { equal } else { !equal });
    }
    let actual = actual
        .as_f64()
        .ok_or_else(|| format!("operator '{operator}' requires a numeric actual value"))?;
    let expected = expected
        .as_f64()
        .ok_or_else(|| format!("operator '{operator}' requires a numeric expected value"))?;
    match operator {
        "lt" => Ok(actual < expected),
        "lte" => Ok(actual <= expected),
        "gt" => Ok(actual > expected),
        "gte" => Ok(actual >= expected),
        _ => Err(format!(
            "unsupported validation operator '{operator}'; use eq, ne, lt, lte, gt, or gte"
        )),
    }
}

const fn one_tick() -> u32 {
    1
}

const fn default_completion_limit() -> usize {
    32
}

const fn default_inspect_limit() -> usize {
    32
}

const fn default_symbol_page_limit() -> usize {
    32
}

const fn default_reference_limit() -> usize {
    128
}

fn default_validation_operator() -> String {
    "eq".to_string()
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveEditOperation {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveEdit {
    pub operation: LiveEditOperation,
    pub target: LiveSymbolTarget,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub expected_source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSymbolTarget {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveRuntimeIdentity {
    pub session_id: String,
    pub generation: u64,
    pub source_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub indexed_collections: Vec<LiveIndexedCollection>,
    #[serde(default = "default_true")]
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveIndexedCollection {
    pub path: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveResponse {
    pub schema_version: u16,
    pub request_id: u64,
    pub tick: u64,
    pub ok: bool,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_identity: Option<LiveRuntimeIdentity>,
}

impl LiveResponse {
    pub fn success(request_id: u64, tick: u64, kind: impl Into<String>, data: Value) -> Self {
        Self {
            schema_version: LIVE_SCHEMA_VERSION,
            request_id,
            tick,
            ok: true,
            kind: kind.into(),
            data: Some(data),
            error: None,
            truncated: false,
            runtime_identity: None,
        }
    }

    pub fn failure(request_id: u64, tick: u64, error: impl Into<String>) -> Self {
        Self {
            schema_version: LIVE_SCHEMA_VERSION,
            request_id,
            tick,
            ok: false,
            kind: "error".to_string(),
            data: None,
            error: Some(error.into()),
            truncated: false,
            runtime_identity: None,
        }
    }

    pub fn with_runtime_identity(mut self, identity: LiveRuntimeIdentity) -> Self {
        self.runtime_identity = Some(identity);
        self
    }

    pub fn bounded(mut self, max_bytes: usize) -> Self {
        let Ok(encoded) = serde_json::to_vec(&self) else {
            return Self::failure(
                self.request_id,
                self.tick,
                "failed to serialize live response",
            );
        };
        if encoded.len() <= max_bytes {
            return self;
        }
        if self.ok && self.kind == "edit_applied" {
            if let Some(data) = self.data.as_ref() {
                let changed_files = data
                    .pointer("/plan/changed_files")
                    .and_then(Value::as_array)
                    .map(|files| {
                        files
                            .iter()
                            .map(|file| {
                                serde_json::json!({
                                    "file": file.get("file").cloned().unwrap_or(Value::Null),
                                    "before_hash": file.get("before_hash").cloned().unwrap_or(Value::Null),
                                    "after_hash": file.get("after_hash").cloned().unwrap_or(Value::Null),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.data = Some(serde_json::json!({
                    "receipt": data.get("receipt").cloned().unwrap_or(Value::Null),
                    "tests": data.get("tests").cloned().unwrap_or(Value::Null),
                    "plan": {
                        "changed_files": changed_files,
                        "reload": data.pointer("/plan/reload").cloned().unwrap_or(Value::Null),
                    },
                    "swap": data.get("swap").cloned().unwrap_or(Value::Null),
                    "jit_patch": data.get("jit_patch").cloned().unwrap_or(Value::Null),
                    "response_compacted": true,
                    "original_bytes": encoded.len(),
                }));
                self.truncated = true;
                if serde_json::to_vec(&self).is_ok_and(|value| value.len() <= max_bytes) {
                    return self;
                }
            }
        }
        self.data = Some(serde_json::json!({
            "message": "live response exceeded the configured output limit",
            "original_bytes": encoded.len(),
            "limit_bytes": max_bytes,
        }));
        self.error = (!self.ok).then(|| "live response exceeded the output limit".to_string());
        self.truncated = true;
        self
    }
}

pub struct LiveSessionClient {
    requests: Sender<LiveRequest>,
    responses: Receiver<LiveResponse>,
    routing: Arc<LiveSessionRouting>,
    owner_id: u64,
}

pub struct LiveSessionServer {
    requests: Receiver<LiveRequest>,
    routing: Arc<LiveSessionRouting>,
    output_limit: usize,
}

struct LiveSessionRouting {
    next_wire_id: AtomicU64,
    next_owner_id: AtomicU64,
    closed: AtomicBool,
    capacity: usize,
    state: Mutex<LiveSessionRoutingState>,
}

struct LiveSessionRoutingState {
    mailboxes: BTreeMap<u64, Sender<LiveResponse>>,
    routes: BTreeMap<u64, LiveRequestRoute>,
    local_requests: BTreeMap<(u64, u64), u64>,
    primary_owner: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct LiveRequestRoute {
    owner_id: u64,
    caller_request_id: u64,
}

fn response_keeps_request_route(response: &LiveResponse) -> bool {
    matches!(
        response.kind.as_str(),
        "edit_preparing" | "completion_preparing"
    )
}

impl LiveSessionRouting {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            next_wire_id: AtomicU64::new(1),
            next_owner_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            capacity,
            state: Mutex::new(LiveSessionRoutingState {
                mailboxes: BTreeMap::new(),
                routes: BTreeMap::new(),
                local_requests: BTreeMap::new(),
                primary_owner: None,
            }),
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, LiveSessionRoutingState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_id(counter: &AtomicU64, kind: &str) -> Result<u64, String> {
        let mut current = counter.load(Ordering::Relaxed);
        loop {
            if current == 0 {
                return Err(format!("live session {kind} id space is exhausted"));
            }
            let next = current.wrapping_add(1);
            match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(current),
                Err(observed) => current = observed,
            }
        }
    }

    fn register_client(&self) -> (u64, Receiver<LiveResponse>) {
        let owner_id = Self::next_id(&self.next_owner_id, "client")
            .expect("live session client id space is exhausted");
        let (sender, receiver) = bounded(self.capacity);
        if !self.closed.load(Ordering::Acquire) {
            let mut state = self.lock_state();
            if !self.closed.load(Ordering::Acquire) {
                if state.primary_owner.is_none() {
                    state.primary_owner = Some(owner_id);
                }
                state.mailboxes.insert(owner_id, sender);
                return (owner_id, receiver);
            }
        }
        drop(sender);
        (owner_id, receiver)
    }

    fn unregister_client(&self, owner_id: u64) {
        let mut state = self.lock_state();
        state.mailboxes.remove(&owner_id);
        state.routes.retain(|_, route| route.owner_id != owner_id);
        state
            .local_requests
            .retain(|(route_owner, _), _| *route_owner != owner_id);
        if state.primary_owner == Some(owner_id) {
            state.primary_owner = state.mailboxes.keys().next().copied();
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let mut state = self.lock_state();
        state.routes.clear();
        state.local_requests.clear();
        state.primary_owner = None;
        state.mailboxes.clear();
    }

    fn submit(
        &self,
        owner_id: u64,
        requests: &Sender<LiveRequest>,
        mut request: LiveRequest,
    ) -> Result<(), String> {
        request.validate()?;
        let wire_id = Self::next_id(&self.next_wire_id, "request")?;
        let caller_request_id = request.request_id;
        let mut state = self.lock_state();
        if self.closed.load(Ordering::Acquire) || !state.mailboxes.contains_key(&owner_id) {
            return Err("live session has ended".to_string());
        }
        if state
            .local_requests
            .contains_key(&(owner_id, caller_request_id))
        {
            return Err(format!(
                "live request_id {caller_request_id} is already pending for this client"
            ));
        }
        let owner_outstanding = state
            .local_requests
            .keys()
            .filter(|(route_owner, _)| *route_owner == owner_id)
            .count();
        if owner_outstanding >= MAX_LIVE_OUTSTANDING_REQUESTS_PER_CLIENT {
            return Err(
                "live-session outstanding request limit reached for this client".to_string(),
            );
        }
        if state.routes.len() >= MAX_LIVE_OUTSTANDING_REQUESTS_PER_SESSION {
            return Err("live-session outstanding request limit reached".to_string());
        }
        if let LiveCommand::Cancel { request_id } = &mut request.command {
            if let Some(target_wire_id) = state.local_requests.get(&(owner_id, *request_id)) {
                *request_id = *target_wire_id;
            } else {
                *request_id = 0;
            }
        }
        request.request_id = wire_id;
        request.validate()?;
        match requests.try_send(request) {
            Ok(()) => {
                state.routes.insert(
                    wire_id,
                    LiveRequestRoute {
                        owner_id,
                        caller_request_id,
                    },
                );
                state
                    .local_requests
                    .insert((owner_id, caller_request_id), wire_id);
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err("live-session command queue is full".to_string()),
            Err(TrySendError::Disconnected(_)) => Err("live session has ended".to_string()),
        }
    }

    fn respond(
        &self,
        response: LiveResponse,
        output_limit: usize,
    ) -> Result<(), LiveResponseSendError> {
        let mut response = response.bounded(output_limit);
        let wire_id = response.request_id;
        let keep_route = response_keeps_request_route(&response);
        let mut state = self.lock_state();
        if self.closed.load(Ordering::Acquire) {
            return Err(LiveResponseSendError::Disconnected);
        }

        let (owner_id, caller_request_id, sender) = if wire_id == 0 {
            let owner_id = state
                .primary_owner
                .filter(|owner_id| state.mailboxes.contains_key(owner_id))
                .or_else(|| state.mailboxes.keys().next().copied());
            let Some(owner_id) = owner_id else {
                return Err(LiveResponseSendError::Disconnected);
            };
            state.primary_owner = Some(owner_id);
            let sender = state
                .mailboxes
                .get(&owner_id)
                .cloned()
                .expect("primary mailbox exists while routing is locked");
            (owner_id, None, sender)
        } else if let Some(route) = state.routes.get(&wire_id).copied() {
            let Some(sender) = state.mailboxes.get(&route.owner_id).cloned() else {
                state.routes.remove(&wire_id);
                state
                    .local_requests
                    .remove(&(route.owner_id, route.caller_request_id));
                return Err(LiveResponseSendError::Disconnected);
            };
            (route.owner_id, Some(route.caller_request_id), sender)
        } else if !state.mailboxes.is_empty() {
            // A response for a dropped or already-completed task is stale.
            // Drop it without taking down the other live clients.
            return Ok(());
        } else {
            return Err(LiveResponseSendError::Disconnected);
        };

        let wire_response = response.clone();
        if let Some(caller_request_id) = caller_request_id {
            response.request_id = caller_request_id;
            response = response.bounded(output_limit);
        }
        match sender.try_send(response) {
            Ok(()) => {
                if caller_request_id.is_some() && !keep_route {
                    let route = state
                        .routes
                        .remove(&wire_id)
                        .expect("request route exists until response delivery");
                    state
                        .local_requests
                        .remove(&(owner_id, route.caller_request_id));
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(LiveResponseSendError::Full(wire_response)),
            Err(TrySendError::Disconnected(_)) => {
                if caller_request_id.is_some() && !keep_route {
                    let route = state.routes.remove(&wire_id);
                    if let Some(route) = route {
                        state
                            .local_requests
                            .remove(&(route.owner_id, route.caller_request_id));
                    }
                }
                Err(LiveResponseSendError::Disconnected)
            }
        }
    }
}

impl Clone for LiveSessionClient {
    fn clone(&self) -> Self {
        let (owner_id, responses) = self.routing.register_client();
        Self {
            requests: self.requests.clone(),
            responses,
            routing: Arc::clone(&self.routing),
            owner_id,
        }
    }
}

impl Drop for LiveSessionClient {
    fn drop(&mut self) {
        self.routing.unregister_client(self.owner_id);
    }
}

impl Drop for LiveSessionServer {
    fn drop(&mut self) {
        self.routing.close();
    }
}

#[derive(Debug)]
pub enum LiveResponseSendError {
    Full(LiveResponse),
    Disconnected,
}

pub fn live_session(capacity: usize) -> (LiveSessionClient, LiveSessionServer) {
    let capacity = capacity.max(1);
    let routing = LiveSessionRouting::new(capacity);
    let (request_tx, request_rx) = bounded(capacity);
    let (owner_id, response_rx) = routing.register_client();
    (
        LiveSessionClient {
            requests: request_tx,
            responses: response_rx,
            routing: Arc::clone(&routing),
            owner_id,
        },
        LiveSessionServer {
            requests: request_rx,
            routing,
            output_limit: DEFAULT_LIVE_OUTPUT_BYTES,
        },
    )
}

impl LiveSessionClient {
    /// Select this client for future session-wide events with request ID zero.
    /// Request replies stay with their submitting client. Cloning does not
    /// transfer this role; dropping the recipient selects the oldest survivor.
    pub fn claim_unsolicited_responses(&self) {
        let mut state = self.routing.lock_state();
        if state.mailboxes.contains_key(&self.owner_id) {
            state.primary_owner = Some(self.owner_id);
        }
    }

    pub fn submit(&self, request: LiveRequest) -> Result<(), String> {
        self.routing.submit(self.owner_id, &self.requests, request)
    }

    pub fn receive_timeout(&self, timeout: Duration) -> Result<LiveResponse, String> {
        self.responses
            .recv_timeout(timeout)
            .map_err(|error| format!("live-session response unavailable: {error}"))
    }

    pub fn try_receive(&self) -> Result<Option<LiveResponse>, String> {
        match self.responses.try_recv() {
            Ok(response) => Ok(Some(response)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("live session has ended".to_string()),
        }
    }
}

impl LiveSessionServer {
    pub fn set_output_limit(&mut self, bytes: usize) {
        self.output_limit = bytes.max(256);
    }

    pub fn caller_request_id(&self, wire_request_id: u64) -> Option<u64> {
        self.routing
            .lock_state()
            .routes
            .get(&wire_request_id)
            .map(|route| route.caller_request_id)
    }

    pub fn drain(&self, limit: usize) -> Vec<LiveRequest> {
        let mut requests = Vec::new();
        while requests.len() < limit {
            match self.requests.try_recv() {
                Ok(request) => requests.push(request),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        requests
    }

    pub fn respond(&self, response: LiveResponse) -> Result<(), LiveResponseSendError> {
        self.routing.respond(response, self.output_limit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub text: String,
    pub kind: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<LiveSymbolTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<CompletionScope>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionScope {
    pub owner: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_end: Option<usize>,
    pub visible_from: usize,
    pub visible_to: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionQuery {
    pub replacement_start: usize,
    pub replacement_end: usize,
    pub page: u32,
    pub truncated: bool,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, Default)]
pub struct CompletionIndex {
    items: Vec<IndexedCompletionItem>,
}

#[derive(Debug, Clone)]
struct IndexedCompletionItem {
    normalized_text: String,
    item: CompletionItem,
}

impl CompletionIndex {
    pub fn replace<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = CompletionItem>,
    {
        let mut items = items.into_iter().collect::<Vec<_>>();
        items.sort_by(|left, right| completion_item_key(left).cmp(&completion_item_key(right)));
        items.dedup();
        self.items = items
            .into_iter()
            .map(|item| IndexedCompletionItem {
                normalized_text: item.text.to_ascii_lowercase(),
                item,
            })
            .collect();
    }

    pub fn extend<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = CompletionItem>,
    {
        let merged = self
            .items
            .iter()
            .map(|indexed| indexed.item.clone())
            .chain(items)
            .collect::<Vec<_>>();
        self.replace(merged);
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&CompletionItem) -> bool) {
        self.items.retain(|indexed| keep(&indexed.item));
    }

    pub fn complete(&self, buffer: &str, cursor: usize, limit: usize) -> Vec<CompletionItem> {
        self.query(buffer, cursor, limit).items
    }

    pub fn query(&self, buffer: &str, cursor: usize, limit: usize) -> CompletionQuery {
        self.query_with_context(buffer, cursor, limit, &CompletionContext::default())
    }

    pub fn query_with_context(
        &self,
        buffer: &str,
        cursor: usize,
        limit: usize,
        context: &CompletionContext,
    ) -> CompletionQuery {
        rank_indexed_completion_items(&self.items, buffer, cursor, limit, context)
    }
}

pub fn rank_completion_items(
    items: &[CompletionItem],
    buffer: &str,
    cursor: usize,
    limit: usize,
) -> CompletionQuery {
    rank_completion_items_with_context(items, buffer, cursor, limit, &CompletionContext::default())
}

pub fn rank_completion_items_with_context(
    items: &[CompletionItem],
    buffer: &str,
    cursor: usize,
    limit: usize,
    context: &CompletionContext,
) -> CompletionQuery {
    let indexed = items
        .iter()
        .cloned()
        .map(|item| IndexedCompletionItem {
            normalized_text: item.text.to_ascii_lowercase(),
            item,
        })
        .collect::<Vec<_>>();
    rank_indexed_completion_items(&indexed, buffer, cursor, limit, context)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CompletionRank<'a> {
    match_class: u8,
    scope_distance: usize,
    expected_type: u8,
    kind: u8,
    gaps: usize,
    first: usize,
    text_len: usize,
    text: &'a str,
    item_kind: &'a str,
    detail: &'a str,
}

fn rank_indexed_completion_items(
    items: &[IndexedCompletionItem],
    buffer: &str,
    cursor: usize,
    limit: usize,
    context: &CompletionContext,
) -> CompletionQuery {
    let mut cursor = cursor.min(buffer.len());
    while !buffer.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let replacement_start = completion_replacement_start(buffer, cursor);
    let query = &buffer[replacement_start..cursor];
    let normalized_query = query.to_ascii_lowercase();
    let bounded_limit = limit.min(256);
    let mut unscoped_match_count = 0usize;
    let mut best = Vec::<(CompletionRank<'_>, &CompletionItem)>::with_capacity(bounded_limit);
    let mut scoped_matches =
        BTreeMap::<(&str, &str, Option<&str>), (CompletionRank<'_>, &CompletionItem)>::new();
    for indexed in items {
        let Some(scope_distance) = completion_scope_distance(&indexed.item, context) else {
            continue;
        };
        let Some((match_class, gaps, first)) =
            fuzzy_completion_score(&indexed.normalized_text, &normalized_query)
        else {
            continue;
        };
        let rank = CompletionRank {
            match_class,
            scope_distance,
            expected_type: completion_expected_type_priority(&indexed.item, context),
            kind: completion_kind_priority(&indexed.item.kind, query),
            gaps,
            first,
            text_len: indexed.item.text.len(),
            text: &indexed.item.text,
            item_kind: &indexed.item.kind,
            detail: &indexed.item.detail,
        };
        if indexed.item.scope.is_some() {
            let overload_detail =
                (indexed.item.kind == "method").then_some(indexed.item.detail.as_str());
            let key = (
                indexed.item.text.as_str(),
                indexed.item.kind.as_str(),
                overload_detail,
            );
            match scoped_matches.get_mut(&key) {
                Some(best) if rank < best.0 => *best = (rank, &indexed.item),
                Some(_) => {}
                None => {
                    scoped_matches.insert(key, (rank, &indexed.item));
                }
            }
        } else {
            unscoped_match_count = unscoped_match_count.saturating_add(1);
            retain_bounded_completion(&mut best, (rank, &indexed.item), bounded_limit);
        }
    }
    let total_matches = unscoped_match_count.saturating_add(scoped_matches.len());
    for candidate in scoped_matches.into_values() {
        retain_bounded_completion(&mut best, candidate, bounded_limit);
    }
    best.sort_by_key(|(rank, _)| *rank);
    let items = best.into_iter().map(|(_, item)| item.clone()).collect();
    CompletionQuery {
        replacement_start,
        replacement_end: cursor,
        page: 0,
        truncated: total_matches > bounded_limit,
        items,
    }
}

fn retain_bounded_completion<'a>(
    best: &mut Vec<(CompletionRank<'a>, &'a CompletionItem)>,
    candidate: (CompletionRank<'a>, &'a CompletionItem),
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    if best.len() < limit {
        best.push(candidate);
        return;
    }
    let (worst_index, worst) = best
        .iter()
        .enumerate()
        .max_by_key(|(_, (rank, _))| *rank)
        .expect("bounded completion set is not empty");
    if candidate.0 < worst.0 {
        best[worst_index] = candidate;
    }
}

fn completion_item_key(
    item: &CompletionItem,
) -> (
    &str,
    &str,
    &str,
    Option<&str>,
    Option<&str>,
    Option<(&str, &str, Option<&str>, Option<usize>, usize, usize)>,
) {
    (
        &item.text,
        &item.kind,
        &item.detail,
        item.type_name.as_deref(),
        item.source.as_deref(),
        item.scope.as_ref().map(|scope| {
            (
                scope.owner.as_str(),
                scope.file.as_str(),
                scope.owner_signature.as_deref(),
                scope.owner_end,
                scope.visible_from,
                scope.visible_to,
            )
        }),
    )
}

fn completion_replacement_start(buffer: &str, cursor: usize) -> usize {
    buffer[..cursor]
        .char_indices()
        .rev()
        .find(|(index, character)| {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | ')'
                        | '['
                        | ']'
                        | ','
                        | ';'
                        | '{'
                        | '}'
                        | '='
                        | '+'
                        | '-'
                        | '*'
                        | '/'
                        | '%'
                        | '<'
                        | '>'
                        | '!'
                )
                || (*character == ':' && *index > 0)
        })
        .map_or(0, |(index, character)| index + character.len_utf8())
}

fn fuzzy_completion_score(text: &str, query: &str) -> Option<(u8, usize, usize)> {
    if text == query {
        return Some((0, 0, 0));
    }
    if text.starts_with(&query) {
        return Some((1, 0, text.len().saturating_sub(query.len())));
    }
    if let Some(start) = text.find(&query) {
        return Some((2, start, text.len().saturating_sub(query.len())));
    }
    let mut text_indices = text.char_indices();
    let mut first = None;
    let mut previous = None;
    let mut gaps = 0usize;
    for needle in query.chars() {
        let (index, _) = text_indices.find(|(_, candidate)| *candidate == needle)?;
        first.get_or_insert(index);
        if let Some(previous) = previous {
            gaps += index.saturating_sub(previous + 1);
        }
        previous = Some(index);
    }
    Some((3, gaps, first.unwrap_or(0)))
}

fn completion_scope_distance(item: &CompletionItem, context: &CompletionContext) -> Option<usize> {
    let Some(scope) = item.scope.as_ref() else {
        return Some(usize::MAX);
    };
    if context.owner.as_deref() != Some(scope.owner.as_str()) {
        return None;
    }
    if context
        .file
        .as_deref()
        .is_some_and(|file| file != scope.file)
    {
        return None;
    }
    if context
        .owner_signature
        .as_deref()
        .is_some_and(|signature| scope.owner_signature.as_deref() != Some(signature))
    {
        return None;
    }
    let offset = context.source_offset.or(scope.owner_end)?;
    if offset < scope.visible_from || offset > scope.visible_to {
        return None;
    }
    Some(usize::MAX.saturating_sub(scope.visible_from))
}

fn completion_expected_type_priority(item: &CompletionItem, context: &CompletionContext) -> u8 {
    match context.expected_type.as_deref() {
        Some(expected) if item.type_name.as_deref() == Some(expected) => 0,
        Some(_) => 1,
        None => 0,
    }
}

fn completion_kind_priority(kind: &str, query: &str) -> u8 {
    if query.starts_with(':') {
        return u8::from(kind != "command");
    }
    if query.contains('.') {
        return match kind {
            "field" | "state_path" => 0,
            "method" => 1,
            _ => 2,
        };
    }
    match kind {
        "local" => 0,
        "parameter" => 1,
        "field" => 2,
        "method" => 3,
        "function" => 4,
        "global" | "constant" | "state_path" => 5,
        "struct" | "enum" | "enum_variant" => 6,
        "scratch_cell" => 7,
        "command" => 8,
        _ => 9,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchCell {
    pub name: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick: Option<u64>,
}

#[derive(Debug, Default)]
pub struct ScratchWorkspace {
    cells: BTreeMap<String, ScratchCell>,
}

impl ScratchWorkspace {
    pub fn put(&mut self, name: &str, code: String) -> Result<(), String> {
        validate_cell_name(name)?;
        if code.len() > MAX_LIVE_MULTILINE_BYTES {
            return Err(format!(
                "scratch cell exceeds {MAX_LIVE_MULTILINE_BYTES} bytes"
            ));
        }
        if !self.cells.contains_key(name) && self.cells.len() >= MAX_SCRATCH_CELLS {
            return Err(format!(
                "scratch workspace is limited to {MAX_SCRATCH_CELLS} cells"
            ));
        }
        self.cells.insert(
            name.to_string(),
            ScratchCell {
                name: name.to_string(),
                code,
                last_result: None,
                last_tick: None,
            },
        );
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ScratchCell> {
        self.cells.get(name)
    }

    pub fn record_result(&mut self, name: &str, tick: u64, result: String) -> Result<(), String> {
        let cell = self
            .cells
            .get_mut(name)
            .ok_or_else(|| format!("scratch cell '{name}' not found"))?;
        cell.last_result = Some(result);
        cell.last_tick = Some(tick);
        Ok(())
    }

    pub fn list(&self) -> Vec<ScratchCell> {
        self.cells.values().cloned().collect()
    }

    pub fn clear(&mut self, name: Option<&str>) -> Result<(), String> {
        if let Some(name) = name {
            if self.cells.remove(name).is_none() {
                return Err(format!("scratch cell '{name}' not found"));
            }
        } else {
            self.cells.clear();
        }
        Ok(())
    }
}

fn validate_cell_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(format!("invalid scratch cell name '{name}'"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    Request(LiveRequest),
    Continue { prompt: &'static str },
}

#[derive(Debug, Default)]
pub struct TerminalBuffer {
    next_request_id: u64,
    pending: Option<PendingMultiline>,
}

#[derive(Debug)]
struct PendingMultiline {
    request_id: u64,
    command: PendingCommand,
    lines: Vec<String>,
    bytes: usize,
}

#[derive(Debug)]
enum PendingCommand {
    Edit {
        operation: LiveEditOperation,
        target: LiveSymbolTarget,
        preview: bool,
    },
    Do {
        preview: bool,
    },
    CellPut {
        name: String,
    },
}

impl TerminalBuffer {
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            pending: None,
        }
    }

    pub fn feed_line(&mut self, line: &str) -> Result<TerminalInput, String> {
        if let Some(pending) = self.pending.as_mut() {
            if line.trim() == ":abort" {
                self.pending = None;
                return Ok(TerminalInput::Continue { prompt: "stasis> " });
            }
            if line.trim() != ":end" {
                if pending.bytes.saturating_add(line.len()).saturating_add(1)
                    > MAX_LIVE_MULTILINE_BYTES
                {
                    self.pending = None;
                    return Err(format!(
                        "multiline input exceeds {MAX_LIVE_MULTILINE_BYTES} bytes"
                    ));
                }
                pending.lines.push(line.to_string());
                pending.bytes = pending.bytes.saturating_add(line.len()).saturating_add(1);
                return Ok(TerminalInput::Continue { prompt: "... " });
            }
            let pending = self.pending.take().expect("pending command");
            let source = pending.lines.join("\n");
            let command = match pending.command {
                PendingCommand::Edit {
                    operation,
                    target,
                    preview,
                } => LiveCommand::Edit {
                    operation,
                    target,
                    source: (operation != LiveEditOperation::Delete).then_some(source),
                    expected_source_hash: None,
                    preview,
                    run_tests: true,
                },
                PendingCommand::Do { preview } => LiveCommand::Do {
                    code: source,
                    preview,
                },
                PendingCommand::CellPut { name } => LiveCommand::CellPut { name, code: source },
            };
            return Ok(TerminalInput::Request(LiveRequest::new(
                pending.request_id,
                command,
            )));
        }

        if line.trim_start().starts_with('{') {
            let request = serde_json::from_str::<LiveRequest>(line)
                .map_err(|error| format!("invalid live request JSON: {error}"))?;
            request.validate()?;
            self.next_request_id = self
                .next_request_id
                .max(request.request_id.saturating_add(1));
            return Ok(TerminalInput::Request(request));
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let command = parse_terminal_command(line)?;
        if let ParsedTerminalCommand::Pending(command) = command {
            self.pending = Some(PendingMultiline {
                request_id,
                command,
                lines: Vec::new(),
                bytes: 0,
            });
            return Ok(TerminalInput::Continue { prompt: "... " });
        }
        let ParsedTerminalCommand::Ready(command) = command else {
            unreachable!()
        };
        Ok(TerminalInput::Request(LiveRequest::new(
            request_id, command,
        )))
    }

    pub fn cancel_pending(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub fn has_pending_input(&self) -> bool {
        self.pending.is_some()
    }

    pub fn completion_context(&self) -> CompletionContext {
        let Some(PendingMultiline {
            command: PendingCommand::Edit { target, .. },
            ..
        }) = self.pending.as_ref()
        else {
            return CompletionContext::default();
        };
        CompletionContext {
            owner: Some(target.name.clone()),
            file: target.file.clone(),
            owner_signature: target.signature.clone(),
            source_offset: None,
            expected_type: None,
        }
    }
}

enum ParsedTerminalCommand {
    Ready(LiveCommand),
    Pending(PendingCommand),
}

fn parse_terminal_command(line: &str) -> Result<ParsedTerminalCommand, String> {
    let args = split_terminal_args(line)?;
    let Some(command) = args.first().map(String::as_str) else {
        return Err("empty live command".to_string());
    };
    let ready = |command| Ok(ParsedTerminalCommand::Ready(command));
    match command {
        ":help" => ready(LiveCommand::Help),
        ":status" => ready(LiveCommand::Status),
        ":pause" => ready(LiveCommand::Pause),
        ":resume" => ready(LiveCommand::Resume),
        ":quit" => ready(LiveCommand::Quit),
        ":step" => ready(LiveCommand::Step {
            ticks: args
                .get(1)
                .map(|value| parse_u32("ticks", value))
                .transpose()?
                .unwrap_or(1),
        }),
        ":cancel" => ready(LiveCommand::Cancel {
            request_id: required_arg(&args, 1, "request id")?
                .parse::<u64>()
                .map_err(|error| format!("invalid request id: {error}"))?,
        }),
        ":symbols" | ":find" => ready(LiveCommand::Symbols {
            query: args.get(1).filter(|arg| !arg.starts_with("--")).cloned(),
            kind: terminal_selector_value(&args, "--kind"),
            files: terminal_selector_values(&args, "--file"),
            owner: terminal_selector_value(&args, "--owner"),
            page: terminal_selector_value(&args, "--page")
                .map(|value| parse_u32("page", &value))
                .transpose()?
                .unwrap_or(0),
            limit: terminal_selector_value(&args, "--limit")
                .map(|value| parse_u32("limit", &value))
                .transpose()?
                .map(|value| value as usize)
                .unwrap_or_else(default_symbol_page_limit),
        }),
        ":read" => ready(LiveCommand::Read {
            name: required_arg(&args, 1, "symbol name")?.to_string(),
            kind: args.get(2).cloned(),
            file: terminal_selector_value(&args, "--file")
                .or_else(|| args.get(3).filter(|arg| !arg.starts_with("--")).cloned()),
            owner: terminal_selector_value(&args, "--owner"),
            signature: terminal_selector_value(&args, "--signature"),
        }),
        ":references" | ":refs" => ready(LiveCommand::References {
            symbol: required_arg(&args, 1, "symbol")?.to_string(),
            limit: terminal_selector_value(&args, "--limit")
                .map(|value| parse_u32("limit", &value))
                .transpose()?
                .map(|value| value as usize)
                .unwrap_or_else(default_reference_limit),
        }),
        ":diagnostics" | ":problems" => ready(LiveCommand::Diagnostics),
        ":hover" | ":type" => ready(LiveCommand::Hover {
            file: required_arg(&args, 1, "file")?.to_string(),
            offset: required_arg(&args, 2, "byte offset")?
                .parse::<usize>()
                .map_err(|error| format!("invalid byte offset: {error}"))?,
        }),
        ":definition" | ":def" => ready(LiveCommand::Definition {
            file: required_arg(&args, 1, "file")?.to_string(),
            offset: required_arg(&args, 2, "byte offset")?
                .parse::<usize>()
                .map_err(|error| format!("invalid byte offset: {error}"))?,
        }),
        ":organize-imports" | ":organize" => ready(LiveCommand::OrganizeImports {
            file: required_arg(&args, 1, "file")?.to_string(),
        }),
        ":quick-fixes" | ":fixes" => ready(LiveCommand::QuickFixes {
            file: required_arg(&args, 1, "file")?.to_string(),
        }),
        ":inlay-hints" | ":inlays" => ready(LiveCommand::InlayHints {
            file: required_arg(&args, 1, "file")?.to_string(),
        }),
        ":call-hierarchy" | ":calls" => ready(LiveCommand::CallHierarchy {
            file: required_arg(&args, 1, "file")?.to_string(),
            offset: required_arg(&args, 2, "byte offset")?
                .parse::<usize>()
                .map_err(|error| format!("invalid byte offset: {error}"))?,
        }),
        ":type-hierarchy" | ":types" => ready(LiveCommand::TypeHierarchy {
            file: required_arg(&args, 1, "file")?.to_string(),
            offset: required_arg(&args, 2, "byte offset")?
                .parse::<usize>()
                .map_err(|error| format!("invalid byte offset: {error}"))?,
        }),
        ":rename" => ready(LiveCommand::RenamePreview {
            file: required_arg(&args, 1, "file")?.to_string(),
            offset: required_arg(&args, 2, "byte offset")?
                .parse::<usize>()
                .map_err(|error| format!("invalid byte offset: {error}"))?,
            new_name: required_arg(&args, 3, "new name")?.to_string(),
        }),
        ":validate" => ready(LiveCommand::Validate {
            requirement: LiveValidationRequirement {
                path: required_arg(&args, 1, "state path")?.to_string(),
                op: required_arg(&args, 2, "operator")?.to_string(),
                value: parse_terminal_scalar(required_arg(&args, 3, "expected value")?),
            },
            frames: terminal_selector_value(&args, "--frames")
                .map(|value| parse_u32("frames", &value))
                .transpose()?
                .unwrap_or(0),
        }),
        ":complete" => {
            let buffer = args.get(1).cloned().unwrap_or_default();
            ready(LiveCommand::Complete {
                cursor: buffer.len(),
                buffer,
                limit: 32,
                context: CompletionContext::default(),
            })
        }
        ":palette" => ready(LiveCommand::Palette {
            query: args
                .get(1)
                .filter(|value| !value.starts_with("--"))
                .cloned()
                .unwrap_or_default(),
            page: terminal_selector_value(&args, "--page")
                .map(|value| parse_u32("page", &value))
                .transpose()?
                .unwrap_or(0),
            limit: terminal_selector_value(&args, "--limit")
                .map(|value| parse_u32("limit", &value))
                .transpose()?
                .map(|value| value as usize)
                .unwrap_or_else(default_completion_limit),
            context: CompletionContext {
                owner: terminal_selector_value(&args, "--owner"),
                file: terminal_selector_value(&args, "--file"),
                owner_signature: terminal_selector_value(&args, "--signature"),
                source_offset: terminal_selector_value(&args, "--offset")
                    .map(|value| parse_u32("offset", &value))
                    .transpose()?
                    .map(|value| value as usize),
                expected_type: terminal_selector_value(&args, "--expected-type"),
            },
        }),
        ":preview" => ready(LiveCommand::Preview),
        ":apply" => ready(LiveCommand::Apply { run_tests: true }),
        ":changes" => ready(LiveCommand::Changes),
        ":undo" => ready(LiveCommand::Undo { run_tests: true }),
        ":redo" => ready(LiveCommand::Redo { run_tests: true }),
        ":inspect" if args.len() == 1 => ready(LiveCommand::InspectAll {
            limit: default_inspect_limit(),
            concise: false,
            every_ticks: None,
        }),
        ":inspect" => ready(LiveCommand::Inspect {
            path: remaining_args(&args, 1, "state query")?,
        }),
        ":watch" => ready(LiveCommand::Watch {
            path: remaining_args(&args, 1, "state query")?,
        }),
        ":unwatch" => ready(LiveCommand::Unwatch {
            path: args.get(1).cloned(),
        }),
        ":set" => ready(LiveCommand::Set {
            path: required_arg(&args, 1, "state path")?.to_string(),
            expression: required_arg(&args, 2, "expression")?.to_string(),
            preview: args.iter().any(|arg| arg == "--preview"),
        }),
        ":print" => ready(LiveCommand::Print {
            expression: remaining_args(&args, 1, "expression")?,
        }),
        ":do" => Ok(ParsedTerminalCommand::Pending(PendingCommand::Do {
            preview: args.iter().any(|arg| arg == "--preview"),
        })),
        ":cell" if args.get(1).map(String::as_str) == Some("put") => {
            Ok(ParsedTerminalCommand::Pending(PendingCommand::CellPut {
                name: required_arg(&args, 2, "cell name")?.to_string(),
            }))
        }
        ":cell" if args.get(1).map(String::as_str) == Some("run") => ready(LiveCommand::CellRun {
            name: required_arg(&args, 2, "cell name")?.to_string(),
            preview: args.iter().any(|arg| arg == "--preview"),
        }),
        ":cell" if args.get(1).map(String::as_str) == Some("list") => ready(LiveCommand::CellList),
        ":cell" if args.get(1).map(String::as_str) == Some("clear") => {
            ready(LiveCommand::CellClear {
                name: args.get(2).cloned(),
            })
        }
        ":cell" if args.get(1).map(String::as_str) == Some("persist") => {
            ready(LiveCommand::CellPersist {
                name: required_arg(&args, 2, "cell name")?.to_string(),
                target: LiveSymbolTarget {
                    kind: Some(required_arg(&args, 3, "symbol kind")?.to_string()),
                    name: required_arg(&args, 4, "symbol name")?.to_string(),
                    file: terminal_selector_value(&args, "--file")
                        .or_else(|| args.get(5).filter(|arg| !arg.starts_with("--")).cloned()),
                    owner: terminal_selector_value(&args, "--owner"),
                    signature: terminal_selector_value(&args, "--signature"),
                },
                preview: args.iter().any(|arg| arg == "--preview"),
                run_tests: true,
            })
        }
        ":add" | ":update" | ":delete" => {
            let operation = match command {
                ":add" => LiveEditOperation::Add,
                ":update" => LiveEditOperation::Update,
                _ => LiveEditOperation::Delete,
            };
            let target = LiveSymbolTarget {
                kind: Some(required_arg(&args, 1, "symbol kind")?.to_string()),
                name: required_arg(&args, 2, "symbol name")?.to_string(),
                file: terminal_selector_value(&args, "--file")
                    .or_else(|| args.get(3).filter(|arg| !arg.starts_with("--")).cloned()),
                owner: terminal_selector_value(&args, "--owner"),
                signature: terminal_selector_value(&args, "--signature"),
            };
            let preview = args.iter().any(|arg| arg == "--preview");
            if operation == LiveEditOperation::Delete {
                ready(LiveCommand::Edit {
                    operation,
                    target,
                    source: None,
                    expected_source_hash: None,
                    preview,
                    run_tests: true,
                })
            } else {
                Ok(ParsedTerminalCommand::Pending(PendingCommand::Edit {
                    operation,
                    target,
                    preview,
                }))
            }
        }
        _ => Err(format!("unknown live command '{command}'; use :help")),
    }
}

fn parse_terminal_scalar(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_string()))
}

fn terminal_selector_value(args: &[String], option: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == option)
        .map(|pair| pair[1].clone())
}

fn terminal_selector_values(args: &[String], option: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == option)
        .map(|pair| pair[1].clone())
        .collect()
}

fn required_arg<'a>(args: &'a [String], index: usize, name: &str) -> Result<&'a str, String> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {name}"))
}

fn remaining_args(args: &[String], index: usize, name: &str) -> Result<String, String> {
    if args.len() <= index {
        return Err(format!("missing {name}"));
    }
    Ok(args[index..].join(" "))
}

fn parse_u32(name: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|error| format!("invalid {name} '{value}': {error}"))
}

fn split_terminal_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in line.trim().chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(expected) = quote {
            if character == expected {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if quote.is_some() {
        return Err("unterminated quoted argument".to_string());
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_has_a_stable_json_contract() {
        let request = LiveRequest::new(
            9,
            LiveCommand::Evaluate {
                expression: "game.enemies[0].hp".into(),
            },
        );
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(json["type"], "evaluate");
        assert_eq!(json["expression"], "game.enemies[0].hp");
        assert_eq!(
            serde_json::from_value::<LiveRequest>(json).expect("round trip"),
            request
        );
    }

    #[test]
    fn edit_batch_has_a_stable_json_contract() {
        let request = LiveRequest::new(
            7,
            LiveCommand::EditBatch {
                edits: vec![LiveEdit {
                    operation: LiveEditOperation::Update,
                    target: LiveSymbolTarget {
                        name: "tick".into(),
                        kind: Some("function".into()),
                        file: Some("src/main.stasis".into()),
                        owner: None,
                        signature: None,
                    },
                    source: Some("function tick(): void {}".into()),
                    expected_source_hash: Some("before".into()),
                }],
                preview: false,
                run_tests: true,
            },
        );
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(json["type"], "edit_batch");
        assert_eq!(json["edits"][0]["operation"], "update");
        assert_eq!(json["edits"][0]["target"]["name"], "tick");
        assert_eq!(
            serde_json::from_value::<LiveRequest>(json).expect("round trip"),
            request
        );
    }

    #[test]
    fn bounded_queue_reports_backpressure_without_blocking() {
        let (client, server) = live_session(1);
        client
            .submit(LiveRequest::new(1, LiveCommand::Status))
            .expect("first request");
        assert_eq!(
            client
                .submit(LiveRequest::new(2, LiveCommand::Status))
                .expect_err("queue should be full"),
            "live-session command queue is full"
        );
        assert_eq!(server.drain(1)[0].request_id, 1);
    }

    #[test]
    fn cloned_clients_have_isolated_mailboxes_and_restore_local_ids() {
        let (client, server) = live_session(4);
        let clone = client.clone();
        client
            .submit(LiveRequest::new(17, LiveCommand::Status))
            .expect("root request");
        clone
            .submit(LiveRequest::new(17, LiveCommand::Status))
            .expect("clone request with the same local id");

        let requests = server.drain(2);
        assert_eq!(requests.len(), 2);
        assert_ne!(requests[0].request_id, requests[1].request_id);

        server
            .respond(LiveResponse::success(
                requests[1].request_id,
                2,
                "clone",
                serde_json::json!({"owner": "clone"}),
            ))
            .expect("clone response");
        server
            .respond(LiveResponse::success(
                requests[0].request_id,
                1,
                "root",
                serde_json::json!({"owner": "root"}),
            ))
            .expect("root response");

        let root_response = client
            .receive_timeout(Duration::from_millis(10))
            .expect("root response delivery");
        assert_eq!(root_response.request_id, 17);
        assert_eq!(
            root_response.data.as_ref().expect("root data")["owner"],
            "root"
        );
        let clone_response = clone
            .receive_timeout(Duration::from_millis(10))
            .expect("clone response delivery");
        assert_eq!(clone_response.request_id, 17);
        assert_eq!(
            clone_response.data.as_ref().expect("clone data")["owner"],
            "clone"
        );
        assert_eq!(client.try_receive().expect("root mailbox"), None);
        assert_eq!(clone.try_receive().expect("clone mailbox"), None);
    }

    #[test]
    fn unsolicited_owner_transfer_preserves_request_routing_and_backpressure() {
        let (client, server) = live_session(1);
        let events = client.clone();
        client
            .submit(LiveRequest::new(17, LiveCommand::Status))
            .unwrap();
        let request = server.drain(1).pop().unwrap();
        events.claim_unsolicited_responses();
        let background = client.clone();

        server
            .respond(LiveResponse::success(
                0,
                1,
                "watch",
                serde_json::json!({"path": "score", "value": 1}),
            ))
            .unwrap();
        let error = LiveResponse::success(
            0,
            2,
            "watch_error",
            serde_json::json!({"path": "score", "error": "unavailable"}),
        );
        let Err(LiveResponseSendError::Full(retry)) = server.respond(error) else {
            panic!("the selected event mailbox must remain bounded");
        };
        assert_eq!(events.try_receive().unwrap().unwrap().kind, "watch");
        server.respond(retry).unwrap();
        assert_eq!(events.try_receive().unwrap().unwrap().kind, "watch_error");
        assert!(client.try_receive().unwrap().is_none());
        assert!(background.try_receive().unwrap().is_none());

        server
            .respond(LiveResponse::success(
                request.request_id,
                3,
                "status",
                serde_json::json!({}),
            ))
            .unwrap();
        assert!(events.try_receive().unwrap().is_none());
        assert_eq!(client.try_receive().unwrap().unwrap().request_id, 17);
        drop(events);
        server
            .respond(LiveResponse::success(
                0,
                4,
                "watch",
                serde_json::json!({"path": "score", "value": 4}),
            ))
            .unwrap();
        assert_eq!(client.try_receive().unwrap().unwrap().kind, "watch");
        assert!(background.try_receive().unwrap().is_none());
    }

    #[test]
    fn response_queue_full_keeps_wire_id_for_retry() {
        let (client, server) = live_session(1);
        client
            .submit(LiveRequest::new(41, LiveCommand::Status))
            .expect("first request");
        let first_wire_id = server.drain(1)[0].request_id;
        client
            .submit(LiveRequest::new(42, LiveCommand::Status))
            .expect("second request");
        let second_wire_id = server.drain(1)[0].request_id;

        server
            .respond(LiveResponse::success(
                first_wire_id,
                1,
                "first",
                serde_json::json!({}),
            ))
            .expect("fill response mailbox");
        let full = server
            .respond(LiveResponse::success(
                second_wire_id,
                2,
                "second",
                serde_json::json!({}),
            ))
            .expect_err("bounded response mailbox");
        let LiveResponseSendError::Full(retry) = full else {
            panic!("expected a retryable full response");
        };
        assert_eq!(retry.request_id, second_wire_id);

        assert_eq!(
            client
                .receive_timeout(Duration::from_millis(10))
                .expect("first response")
                .request_id,
            41
        );
        server.respond(retry).expect("retry response");
        assert_eq!(
            client
                .receive_timeout(Duration::from_millis(10))
                .expect("second response")
                .request_id,
            42
        );
    }

    #[test]
    fn preparing_responses_keep_the_route_for_the_terminal_response() {
        let (client, server) = live_session(2);
        client
            .submit(LiveRequest::new(23, LiveCommand::Status))
            .expect("request");
        let wire_id = server.drain(1)[0].request_id;

        server
            .respond(LiveResponse::success(
                wire_id,
                1,
                "edit_preparing",
                serde_json::json!({"background": true}),
            ))
            .expect("preparing response");
        assert_eq!(
            client
                .receive_timeout(Duration::from_millis(10))
                .expect("preparing response delivery")
                .request_id,
            23
        );

        server
            .respond(LiveResponse::success(
                wire_id,
                2,
                "edit_applied",
                serde_json::json!({}),
            ))
            .expect("terminal response");
        assert_eq!(
            client
                .receive_timeout(Duration::from_millis(10))
                .expect("terminal response delivery")
                .request_id,
            23
        );
    }

    #[test]
    fn restored_max_request_id_is_bounded_before_delivery() {
        let (client, mut server) = live_session(1);
        server.set_output_limit(1024);
        client
            .submit(LiveRequest::new(u64::MAX, LiveCommand::Status))
            .expect("request");
        let wire_id = server.drain(1)[0].request_id;
        let response = (0..10_000).find_map(|size| {
            let response = LiveResponse::success(
                wire_id,
                1,
                "status",
                serde_json::json!({"payload": "x".repeat(size)}),
            )
            .bounded(1024);
            let wire_bytes = serde_json::to_vec(&response).expect("wire response").len();
            let mut restored = response.clone();
            restored.request_id = u64::MAX;
            let restored_bytes = serde_json::to_vec(&restored)
                .expect("restored response")
                .len();
            (wire_bytes <= 1024 && restored_bytes > 1024).then_some(response)
        });
        let response = response.expect("find a response near the output bound");

        server.respond(response).expect("bounded response");
        let received = client
            .receive_timeout(Duration::from_millis(10))
            .expect("response delivery");
        assert_eq!(received.request_id, u64::MAX);
        assert!(received.truncated);
        assert!(
            serde_json::to_vec(&received)
                .expect("encoded response")
                .len()
                <= 1024
        );
    }

    #[test]
    fn dropping_a_clone_removes_its_pending_routes() {
        let (client, server) = live_session(2);
        let clone = client.clone();
        clone
            .submit(LiveRequest::new(9, LiveCommand::Status))
            .expect("clone request");
        let wire_id = server.drain(1)[0].request_id;
        drop(clone);

        server
            .respond(LiveResponse::success(
                wire_id,
                1,
                "status",
                serde_json::json!({}),
            ))
            .expect("stale response is discarded while root remains");
        assert_eq!(client.try_receive().expect("root mailbox"), None);
    }

    #[test]
    fn dropping_server_disconnects_client_mailboxes() {
        let (client, server) = live_session(1);
        drop(server);

        assert!(client
            .try_receive()
            .expect_err("closed response mailbox")
            .contains("ended"));
        assert!(client
            .submit(LiveRequest::new(1, LiveCommand::Status))
            .expect_err("closed request queue")
            .contains("ended"));
    }

    #[test]
    fn cancel_targets_are_remapped_within_the_cloning_client() {
        let (client, server) = live_session(4);
        client
            .submit(LiveRequest::new(10, LiveCommand::Status))
            .expect("target request");
        client
            .submit(LiveRequest::new(11, LiveCommand::Cancel { request_id: 10 }))
            .expect("cancel request");

        let requests = server.drain(2);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].command,
            LiveCommand::Cancel {
                request_id: requests[0].request_id
            }
        );
    }

    #[test]
    fn response_output_is_bounded_and_explicitly_truncated() {
        let (client, mut server) = live_session(1);
        server.set_output_limit(256);
        client
            .submit(LiveRequest::new(7, LiveCommand::Status))
            .expect("request");
        let first_wire_id = server.drain(1)[0].request_id;
        server
            .respond(LiveResponse::success(
                first_wire_id,
                9,
                "symbols",
                serde_json::json!({"source": "x".repeat(4096)}),
            ))
            .expect("response");
        let response = client
            .receive_timeout(Duration::from_millis(10))
            .expect("receive");
        assert!(response.truncated);
        assert_eq!(response.request_id, 7);
        assert_eq!(response.tick, 9);

        client
            .submit(LiveRequest::new(8, LiveCommand::Status))
            .expect("failure request");
        let second_wire_id = server.drain(1)[0].request_id;
        server
            .respond(LiveResponse::failure(second_wire_id, 10, "x".repeat(4096)))
            .expect("failure response");
        let failure = client
            .receive_timeout(Duration::from_millis(10))
            .expect("receive failure");
        assert!(failure.truncated);
        assert_eq!(
            failure.error.as_deref(),
            Some("live response exceeded the output limit")
        );
        assert!(serde_json::to_vec(&failure).expect("encode").len() <= 256);
    }

    #[test]
    fn oversized_applied_edit_preserves_reload_and_receipt_evidence() {
        let response = LiveResponse::success(
            8,
            10,
            "edit_applied",
            serde_json::json!({
                "receipt": "build/live-edits/receipt.json",
                "tests": "passed",
                "plan": {
                    "changed_files": [{
                        "file": "src/main.stasis",
                        "before_hash": "before",
                        "after_hash": "after",
                        "before_source": "x".repeat(4096),
                        "after_source": "y".repeat(4096),
                    }],
                    "reload": {"expected_reload": "ResetRequired"},
                },
                "swap": {"state_layout_compatible": true},
                "jit_patch": {"revision": 7},
            }),
        )
        .bounded(2048);

        assert!(response.truncated);
        let data = response.data.expect("compacted data");
        assert_eq!(
            data.pointer("/plan/reload/expected_reload"),
            Some(&Value::String("ResetRequired".to_string()))
        );
        assert_eq!(
            data.get("receipt").and_then(Value::as_str),
            Some("build/live-edits/receipt.json")
        );
        assert_eq!(
            data.pointer("/plan/changed_files/0/file")
                .and_then(Value::as_str),
            Some("src/main.stasis")
        );
        assert!(data.pointer("/plan/changed_files/0/after_source").is_none());
    }

    #[test]
    fn terminal_multiline_edit_preserves_inline_source() {
        let mut terminal = TerminalBuffer::new();
        assert!(matches!(
            terminal
                .feed_line(":update function tick src/main.stasis")
                .expect("start"),
            TerminalInput::Continue { .. }
        ));
        terminal
            .feed_line("function tick(): i32 {")
            .expect("body one");
        terminal.feed_line("    return 2;").expect("body two");
        terminal.feed_line("}").expect("body three");
        let TerminalInput::Request(request) = terminal.feed_line(":end").expect("finish") else {
            panic!("expected request")
        };
        let LiveCommand::Edit { source, target, .. } = request.command else {
            panic!("expected edit")
        };
        assert_eq!(target.name, "tick");
        assert_eq!(target.file.as_deref(), Some("src/main.stasis"));
        assert!(source.expect("source").contains("return 2"));
    }

    #[test]
    fn completion_uses_deterministic_prefix_ranking_and_deduplication() {
        let mut index = CompletionIndex::default();
        index.replace([
            CompletionItem {
                text: "tick".into(),
                kind: "function".into(),
                detail: "tick(): i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "tick".into(),
                kind: "function".into(),
                detail: "tick(): i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "tick".into(),
                kind: "function".into(),
                detail: "tick(value: i32): i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "ticker".into(),
                kind: "global".into(),
                detail: "i32".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "title".into(),
                kind: "global".into(),
                detail: "utf8[32]".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
        ]);
        let result = index.complete(":read tick", 10, 10);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].text, "tick");
        assert_eq!(result[1].text, "tick");
        assert_eq!(result[2].text, "ticker");
        assert_eq!(index.complete("éti", 1, 10).len(), 4);
    }

    #[test]
    fn completion_query_fuzzy_ranks_context_and_reports_replacement_range() {
        let mut index = CompletionIndex::default();
        index.replace([
            CompletionItem {
                text: "hero.hp".into(),
                kind: "field".into(),
                detail: "i32 via local hero: Player".into(),
                type_name: Some("i32".into()),
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: "hero.damage".into(),
                kind: "method".into(),
                detail: "damage(player: Player, amount: i32): i32".into(),
                type_name: Some("i32".into()),
                source: None,
                selector: None,
                scope: None,
            },
            CompletionItem {
                text: ":help".into(),
                kind: "command".into(),
                detail: "live command".into(),
                type_name: None,
                source: None,
                selector: None,
                scope: None,
            },
        ]);
        let member = index.query("call(hrohp", 10, 10);
        assert_eq!(member.replacement_start, 5);
        assert_eq!(member.replacement_end, 10);
        assert_eq!(member.items[0].text, "hero.hp");
        assert!(!member.truncated);

        let commands = index.query(":h", 2, 1);
        assert_eq!(commands.items[0].text, ":help");
        assert!(!commands.truncated);

        let bounded = index.query("hero.", 5, 1);
        assert_eq!(bounded.items.len(), 1);
        assert!(bounded.truncated);
    }

    #[test]
    fn completion_query_filters_scopes_shadowing_and_expected_types() {
        let scoped =
            |detail: &str, owner: &str, from: usize, to: usize, owner_end: usize| CompletionItem {
                text: "value".into(),
                kind: "local".into(),
                detail: detail.into(),
                type_name: Some(if detail == "inner" { "i32" } else { "f32" }.into()),
                source: Some("src/main.stasis".into()),
                selector: None,
                scope: Some(CompletionScope {
                    owner: owner.into(),
                    file: "src/main.stasis".into(),
                    owner_signature: Some(format!("{owner}(): i32")),
                    owner_end: Some(owner_end),
                    visible_from: from,
                    visible_to: to,
                }),
            };
        let mut index = CompletionIndex::default();
        index.replace([
            scoped("outer", "tick", 10, 100, 100),
            scoped("inner", "tick", 50, 70, 100),
            scoped("render local", "render", 20, 90, 90),
        ]);
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: None,
            source_offset: Some(60),
            expected_type: Some("i32".into()),
        };
        let result = index.query_with_context("val", 3, 10, &context);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].detail, "inner");

        let target_end = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            ..CompletionContext::default()
        };
        let result = index.query_with_context("val", 3, 10, &target_end);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].detail, "outer");

        let mut nested_only = CompletionIndex::default();
        nested_only.replace([scoped("inner", "tick", 50, 70, 100)]);
        assert!(nested_only
            .query_with_context("val", 3, 10, &target_end)
            .items
            .is_empty());

        let render = CompletionContext {
            owner: Some("render".into()),
            source_offset: Some(50),
            ..CompletionContext::default()
        };
        assert_eq!(
            index.query_with_context("val", 3, 10, &render).items[0].detail,
            "render local"
        );
        assert!(index.query("val", 3, 10).items.is_empty());
    }

    #[test]
    fn completion_query_uses_signature_to_disambiguate_overload_scope() {
        let local = |text: &str, signature: &str| CompletionItem {
            text: text.into(),
            kind: "local".into(),
            detail: signature.into(),
            type_name: Some("i32".into()),
            source: Some("src/main.stasis".into()),
            selector: None,
            scope: Some(CompletionScope {
                owner: "update".into(),
                file: "src/main.stasis".into(),
                owner_signature: Some(signature.into()),
                owner_end: Some(100),
                visible_from: 10,
                visible_to: 100,
            }),
        };
        let mut index = CompletionIndex::default();
        index.replace([
            local("player_value", "update(player: Player): i32"),
            local("enemy_value", "update(enemy: Enemy): i32"),
        ]);
        let context = CompletionContext {
            owner: Some("update".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: Some("update(player: Player): i32".into()),
            source_offset: None,
            expected_type: None,
        };
        let result = index.query_with_context("value", 5, 10, &context);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].text, "player_value");
    }

    #[test]
    fn completion_query_deduplicates_shadowing_before_bounding() {
        let scoped = |detail: &str, from: usize| CompletionItem {
            text: "value".into(),
            kind: "local".into(),
            detail: detail.into(),
            type_name: Some("i32".into()),
            source: Some("src/main.stasis".into()),
            selector: None,
            scope: Some(CompletionScope {
                owner: "tick".into(),
                file: "src/main.stasis".into(),
                owner_signature: Some("tick(): i32".into()),
                owner_end: Some(100),
                visible_from: from,
                visible_to: 100,
            }),
        };
        let mut index = CompletionIndex::default();
        index.replace([
            scoped("outer", 10),
            scoped("inner", 50),
            CompletionItem {
                text: "valid".into(),
                kind: "function".into(),
                detail: "valid(): i32".into(),
                type_name: Some("i32".into()),
                source: None,
                selector: None,
                scope: None,
            },
        ]);
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: Some("tick(): i32".into()),
            source_offset: Some(60),
            expected_type: None,
        };
        let result = index.query_with_context("va", 2, 2, &context);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].detail, "inner");
        assert_eq!(result.items[1].text, "valid");
        assert!(!result.truncated);
    }

    #[test]
    fn completion_query_preserves_scoped_method_overloads() {
        let method = |detail: &str| CompletionItem {
            text: "hero.damage".into(),
            kind: "method".into(),
            detail: detail.into(),
            type_name: Some("i32".into()),
            source: Some("src/main.stasis".into()),
            selector: None,
            scope: Some(CompletionScope {
                owner: "tick".into(),
                file: "src/main.stasis".into(),
                owner_signature: Some("tick(): i32".into()),
                owner_end: Some(100),
                visible_from: 10,
                visible_to: 100,
            }),
        };
        let mut index = CompletionIndex::default();
        index.replace([
            method("damage(hero: Player, amount: i32): i32"),
            method("damage(hero: Player, amount: f32): i32"),
        ]);
        let context = CompletionContext {
            owner: Some("tick".into()),
            file: Some("src/main.stasis".into()),
            owner_signature: Some("tick(): i32".into()),
            source_offset: None,
            expected_type: None,
        };
        let result = index.query_with_context("hrodam", 6, 10, &context);
        assert_eq!(result.items.len(), 2);
        assert_ne!(result.items[0].detail, result.items[1].detail);
    }

    #[test]
    fn completion_query_reaches_late_catalog_items_with_bounded_top_k() {
        let mut items = (0..10_000)
            .map(|index| CompletionItem {
                text: format!("symbol_{index:05}"),
                kind: "function".into(),
                detail: format!("symbol_{index:05}(): i32"),
                type_name: Some("i32".into()),
                source: Some("src/generated.stasis".into()),
                selector: None,
                scope: None,
            })
            .collect::<Vec<_>>();
        items.push(CompletionItem {
            text: "very_late_target".into(),
            kind: "function".into(),
            detail: "very_late_target(): i32".into(),
            type_name: Some("i32".into()),
            source: Some("src/late.stasis".into()),
            selector: None,
            scope: None,
        });
        let mut index = CompletionIndex::default();
        index.replace(items);
        let target = index.query("vltgt", 5, 8);
        assert_eq!(target.items[0].text, "very_late_target");
        let bounded = index.query("symbol", 6, 8);
        assert_eq!(bounded.items.len(), 8);
        assert!(bounded.truncated);
    }

    #[test]
    fn completion_replacement_starts_after_unspaced_infix_operators() {
        let mut index = CompletionIndex::default();
        index.replace([CompletionItem {
            text: "player".into(),
            kind: "parameter".into(),
            detail: "Player".into(),
            type_name: Some("Player".into()),
            source: None,
            selector: None,
            scope: None,
        }]);
        for buffer in ["score+pla", "x!=pla", "value*pla", "items[pla"] {
            let result = index.query(buffer, buffer.len(), 4);
            assert_eq!(result.items[0].text, "player", "{buffer}");
            assert_eq!(&buffer[result.replacement_start..], "pla", "{buffer}");
        }
        assert_eq!(index.query(":hel", 4, 4).replacement_start, 0);
    }

    #[test]
    fn terminal_palette_command_preserves_query_text() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal
            .feed_line(
                ":palette hero.hp --page 2 --limit 12 --owner tick --file src/main.stasis --offset 42 --expected-type i32",
            )
            .expect("palette command")
        else {
            panic!("expected request")
        };
        assert_eq!(
            request.command,
            LiveCommand::Palette {
                query: "hero.hp".to_string(),
                page: 2,
                limit: 12,
                context: CompletionContext {
                    owner: Some("tick".into()),
                    file: Some("src/main.stasis".into()),
                    owner_signature: None,
                    source_offset: Some(42),
                    expected_type: Some("i32".into()),
                },
            }
        );
    }

    #[test]
    fn terminal_bare_inspect_requests_the_default_state_view() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal.feed_line(":inspect").expect("inspect")
        else {
            panic!("expected request")
        };
        assert_eq!(
            request.command,
            LiveCommand::InspectAll {
                limit: 32,
                concise: false,
                every_ticks: None,
            }
        );
    }

    #[test]
    fn terminal_preserves_spaced_state_queries() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(inspect) = terminal
            .feed_line(":inspect enemies[?hp >= score + 1]")
            .expect("inspect query")
        else {
            panic!("expected inspect request")
        };
        assert_eq!(
            inspect.command,
            LiveCommand::Inspect {
                path: "enemies[?hp >= score + 1]".to_string(),
            }
        );
        let TerminalInput::Request(watch) = terminal
            .feed_line(":watch score + enemies[2].hp")
            .expect("watch query")
        else {
            panic!("expected watch request")
        };
        assert_eq!(
            watch.command,
            LiveCommand::Watch {
                path: "score + enemies[2].hp".to_string(),
            }
        );
    }

    #[test]
    fn terminal_completion_context_tracks_pending_semantic_owner() {
        let mut terminal = TerminalBuffer::new();
        terminal
            .feed_line(":update function tick src/main.stasis")
            .expect("start update");
        assert_eq!(
            terminal.completion_context(),
            CompletionContext {
                owner: Some("tick".into()),
                file: Some("src/main.stasis".into()),
                owner_signature: None,
                source_offset: None,
                expected_type: None,
            }
        );
        assert!(terminal.cancel_pending());
        assert_eq!(terminal.completion_context(), CompletionContext::default());
    }

    #[test]
    fn scratch_cells_are_session_only_and_record_tick_stamped_results() {
        let mut scratch = ScratchWorkspace::default();
        scratch
            .put("score", "score = score + 1;".into())
            .expect("put");
        scratch
            .record_result("score", 12, "ok".into())
            .expect("result");
        let cell = scratch.get("score").expect("cell");
        assert_eq!(cell.last_tick, Some(12));
        assert_eq!(cell.last_result.as_deref(), Some("ok"));
        scratch.clear(Some("score")).expect("clear");
        assert!(scratch.list().is_empty());
    }

    #[test]
    fn request_schema_and_ids_are_validated() {
        let (client, _) = live_session(1);
        let mut request = LiveRequest::new(0, LiveCommand::Status);
        assert!(client
            .submit(request.clone())
            .expect_err("zero id")
            .contains("request_id"));
        request.request_id = 1;
        request.schema_version = 99;
        assert!(client
            .submit(request)
            .expect_err("schema")
            .contains("schema version"));

        let oversized = LiveRequest::new(
            2,
            LiveCommand::Do {
                code: "x".repeat(MAX_LIVE_REQUEST_BYTES),
                preview: false,
            },
        );
        assert!(oversized.validate().expect_err("size").contains("limit"));
    }

    #[test]
    fn terminal_json_preserves_protocol_request_id() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal
            .feed_line(r#"{"schema_version":1,"request_id":42,"type":"status"}"#)
            .expect("json request")
        else {
            panic!("expected request")
        };
        assert_eq!(request.request_id, 42);
        assert_eq!(request.command, LiveCommand::Status);
    }

    #[test]
    fn gauntlet_capture_and_input_commands_have_stable_json() {
        let capture: LiveRequest = serde_json::from_str(
            r#"{"schema_version":1,"request_id":70,"type":"capture_frame","artifact":"candidate-0001"}"#,
        )
        .expect("capture request");
        assert_eq!(
            capture.command,
            LiveCommand::CaptureFrame {
                artifact: "candidate-0001".to_string()
            }
        );

        let input: LiveRequest = serde_json::from_str(
            r#"{"schema_version":1,"request_id":71,"type":"set_input_state","pointers":[{"id":0,"x":480,"y":270,"is_down":true,"went_down":true}]}"#,
        )
        .expect("input request");
        assert!(matches!(
            input.command,
            LiveCommand::SetInputState { ref pointers }
                if pointers.len() == 1 && pointers[0].x == 480 && pointers[0].went_down
        ));

        let reinitialize: LiveRequest = serde_json::from_str(
            r#"{"schema_version":1,"request_id":72,"type":"validation_reinitialize"}"#,
        )
        .expect("reinitialize request");
        assert_eq!(reinitialize.command, LiveCommand::ValidationReinitialize);
    }

    #[test]
    fn terminal_multiline_can_be_canceled_and_is_bounded() {
        let mut terminal = TerminalBuffer::new();
        terminal.feed_line(":do").expect("start");
        assert!(terminal.cancel_pending());
        assert!(!terminal.cancel_pending());
        terminal.feed_line(":do").expect("restart");
        assert!(terminal
            .feed_line(&"x".repeat(MAX_LIVE_MULTILINE_BYTES + 1))
            .expect_err("bounded")
            .contains("exceeds"));
        assert!(!terminal.cancel_pending());
    }

    #[test]
    fn terminal_selectors_and_symbol_paging_are_explicit() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(request) = terminal
            .feed_line(
                r#":read update function --file src/game.stasis --owner Enemy --signature "update(i32): void""#,
            )
            .expect("selector")
        else {
            panic!("expected request")
        };
        let LiveCommand::Read {
            file,
            owner,
            signature,
            ..
        } = request.command
        else {
            panic!("expected read")
        };
        assert_eq!(file.as_deref(), Some("src/game.stasis"));
        assert_eq!(owner.as_deref(), Some("Enemy"));
        assert_eq!(signature.as_deref(), Some("update(i32): void"));

        let TerminalInput::Request(request) = terminal
            .feed_line(":symbols update --kind function --file src/game.stasis --file src/enemy.stasis --owner Enemy --page 2 --limit 10")
            .expect("page")
        else {
            panic!("expected request")
        };
        assert!(matches!(
            request.command,
            LiveCommand::Symbols {
                query: Some(ref query),
                kind: Some(ref kind),
                ref files,
                owner: Some(ref owner),
                page: 2,
                limit: 10,
            } if query == "update"
                && kind == "function"
                && files == &["src/game.stasis", "src/enemy.stasis"]
                && owner == "Enemy"
        ));
    }

    #[test]
    fn terminal_exposes_navigation_rename_and_runtime_validation_commands() {
        let mut terminal = TerminalBuffer::new();
        let TerminalInput::Request(references) = terminal
            .feed_line(":references GameState.player_y --limit 24")
            .expect("references")
        else {
            panic!("expected reference request");
        };
        assert_eq!(
            references.command,
            LiveCommand::References {
                symbol: "GameState.player_y".into(),
                limit: 24,
            }
        );

        let TerminalInput::Request(rename) = terminal
            .feed_line(":rename src/game.stasis 42 player_speed")
            .expect("rename preview")
        else {
            panic!("expected rename request");
        };
        assert_eq!(
            rename.command,
            LiveCommand::RenamePreview {
                file: "src/game.stasis".into(),
                offset: 42,
                new_name: "player_speed".into(),
            }
        );

        let TerminalInput::Request(hover) = terminal
            .feed_line(":hover src/game.stasis 24")
            .expect("hover")
        else {
            panic!("expected hover request");
        };
        assert_eq!(
            hover.command,
            LiveCommand::Hover {
                file: "src/game.stasis".into(),
                offset: 24,
            }
        );

        let TerminalInput::Request(definition) = terminal
            .feed_line(":definition src/game.stasis 24")
            .expect("definition")
        else {
            panic!("expected definition request");
        };
        assert_eq!(
            definition.command,
            LiveCommand::Definition {
                file: "src/game.stasis".into(),
                offset: 24,
            }
        );

        let TerminalInput::Request(diagnostics) =
            terminal.feed_line(":diagnostics").expect("diagnostics")
        else {
            panic!("expected diagnostics request");
        };
        assert_eq!(diagnostics.command, LiveCommand::Diagnostics);

        let TerminalInput::Request(organize) = terminal
            .feed_line(":organize-imports src/game.stasis")
            .expect("organize imports")
        else {
            panic!("expected organize-imports request");
        };
        assert_eq!(
            organize.command,
            LiveCommand::OrganizeImports {
                file: "src/game.stasis".into(),
            }
        );

        let TerminalInput::Request(fixes) = terminal
            .feed_line(":quick-fixes src/game.stasis")
            .expect("quick fixes")
        else {
            panic!("expected quick-fixes request");
        };
        assert_eq!(
            fixes.command,
            LiveCommand::QuickFixes {
                file: "src/game.stasis".into(),
            }
        );

        let TerminalInput::Request(inlays) = terminal
            .feed_line(":inlay-hints src/game.stasis")
            .expect("inlay hints")
        else {
            panic!("expected inlay-hints request");
        };
        assert_eq!(
            inlays.command,
            LiveCommand::InlayHints {
                file: "src/game.stasis".into(),
            }
        );

        let TerminalInput::Request(calls) = terminal
            .feed_line(":call-hierarchy src/game.stasis 12")
            .expect("call hierarchy")
        else {
            panic!("expected call-hierarchy request");
        };
        assert_eq!(
            calls.command,
            LiveCommand::CallHierarchy {
                file: "src/game.stasis".into(),
                offset: 12,
            }
        );

        let TerminalInput::Request(types) = terminal
            .feed_line(":type-hierarchy src/game.stasis 8")
            .expect("type hierarchy")
        else {
            panic!("expected type-hierarchy request");
        };
        assert_eq!(
            types.command,
            LiveCommand::TypeHierarchy {
                file: "src/game.stasis".into(),
                offset: 8,
            }
        );

        let TerminalInput::Request(validation) = terminal
            .feed_line(":validate Render.command1_h eq 144 --frames 2")
            .expect("validation")
        else {
            panic!("expected validation request");
        };
        assert_eq!(
            validation.command,
            LiveCommand::Validate {
                requirement: LiveValidationRequirement {
                    path: "Render.command1_h".into(),
                    op: "eq".into(),
                    value: serde_json::json!(144),
                },
                frames: 2,
            }
        );
    }
}
