use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use stasis_language_service::{LiveIndexedCollection, LiveObservation, LiveObservationBatch};

const MAX_PROTOCOL_LINE_BYTES: usize = 16 * 1024 * 1024;
const LIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) const LIVE_EVENT_METHOD: &str = "stasis/liveEvent";
pub(crate) const LIVE_LOG_METHOD: &str = "stasis/liveLog";
pub(crate) const LIVE_STATE_METHOD: &str = "stasis/liveState";

type NotificationSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Debug)]
pub(crate) enum LiveCacheEvent {
    Publish(LiveObservationBatch),
    Clear,
}

pub(crate) type LiveCacheMailbox = Arc<Mutex<Option<LiveCacheEvent>>>;

#[derive(Clone)]
pub(crate) struct LiveProcessBroker {
    inner: Arc<BrokerInner>,
}

struct BrokerInner {
    project_root: PathBuf,
    executable: PathBuf,
    next_request_id: AtomicU64,
    state: Mutex<ProcessState>,
    notify: NotificationSink,
    cache_event: LiveCacheMailbox,
}

#[derive(Default)]
struct ProcessState {
    token: u64,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pending: BTreeMap<u64, mpsc::Sender<Result<Value, String>>>,
    cache: ResponseCache,
}

#[derive(Default)]
struct ResponseCache {
    identity: Option<RuntimeIdentity>,
    observations: BTreeMap<String, LiveObservation>,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeIdentity {
    session_id: String,
    generation: u64,
    source_hashes: BTreeMap<String, String>,
    #[serde(default)]
    indexed_collections: Vec<RuntimeIndexedCollection>,
    #[serde(default)]
    complete: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeIndexedCollection {
    path: String,
    fields: BTreeMap<String, String>,
}

impl LiveProcessBroker {
    pub(crate) fn new(
        project_root: &Path,
        notify: NotificationSink,
    ) -> Result<(Self, LiveCacheMailbox), String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("failed resolving the Stasis executable: {error}"))?;
        let cache_event = Arc::new(Mutex::new(None));
        Ok((
            Self {
                inner: Arc::new(BrokerInner {
                    project_root: project_root.to_path_buf(),
                    executable,
                    next_request_id: AtomicU64::new(1),
                    state: Mutex::new(ProcessState::default()),
                    notify,
                    cache_event: cache_event.clone(),
                }),
            },
            cache_event,
        ))
    }

    pub(crate) fn start(&self, entry: Option<&str>) -> Result<Value, String> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| "live process state lock is poisoned".to_string())?;
        if state.child.is_some() {
            drop(state);
            return self.request(Map::from_iter([(
                "type".to_string(),
                Value::String("status".to_string()),
            )]));
        }

        let mut command = Command::new(&self.inner.executable);
        command
            .current_dir(&self.inner.project_root)
            .arg("--workspace")
            .arg(&self.inner.project_root)
            .arg("tui");
        if let Some(entry) = entry.filter(|entry| !entry.trim().is_empty()) {
            command.arg(entry.trim());
        }
        command
            .arg("--live-stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed launching '{}' for live Workshop: {error}",
                self.inner.executable.display()
            )
        })?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("live Workshop stdin was unavailable".to_string());
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("live Workshop stdout was unavailable".to_string());
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("live Workshop stderr was unavailable".to_string());
        };

        state.token = state.token.wrapping_add(1).max(1);
        state.child = Some(child);
        state.stdin = Some(stdin);
        state.cache = ResponseCache::default();
        let token = state.token;
        drop(state);
        (self.inner.notify)(LIVE_STATE_METHOD, json!({"state": "starting"}));

        let stdout_broker = self.clone();
        if let Err(error) = thread::Builder::new()
            .name("stasis-lsp-live-stdout".to_string())
            .spawn(move || stdout_broker.read_stdout(token, stdout))
        {
            self.end_session(
                token,
                Some(format!("failed starting live stdout reader: {error}")),
            );
            return Err(format!("failed starting live stdout reader: {error}"));
        }
        let stderr_broker = self.clone();
        if let Err(error) = thread::Builder::new()
            .name("stasis-lsp-live-stderr".to_string())
            .spawn(move || stderr_broker.read_stderr(token, stderr))
        {
            self.end_session(
                token,
                Some(format!("failed starting live stderr reader: {error}")),
            );
            return Err(format!("failed starting live stderr reader: {error}"));
        }

        self.request(Map::from_iter([(
            "type".to_string(),
            Value::String("status".to_string()),
        )]))
    }

    pub(crate) fn request(&self, mut fields: Map<String, Value>) -> Result<Value, String> {
        let command = fields
            .get("type")
            .and_then(Value::as_str)
            .filter(|command| !command.is_empty())
            .ok_or_else(|| "live request requires a non-empty 'type'".to_string())?;
        if command == "quit" {
            return Err("use stasis/live/stop to end the LSP-owned session".to_string());
        }
        let request_id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        fields.insert("schema_version".to_string(), json!(1));
        fields.insert("request_id".to_string(), json!(request_id));
        let encoded = serde_json::to_vec(&Value::Object(fields))
            .map_err(|error| format!("failed serializing live request: {error}"))?;
        if encoded.len() > MAX_PROTOCOL_LINE_BYTES {
            return Err("live request exceeds the 16 MiB protocol bound".to_string());
        }

        let (response_tx, response_rx) = mpsc::channel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| "live process state lock is poisoned".to_string())?;
            state.pending.insert(request_id, response_tx);
            let Some(stdin) = state.stdin.as_mut() else {
                state.pending.remove(&request_id);
                return Err("no LSP-owned live Workshop session is running".to_string());
            };
            if let Err(error) = stdin
                .write_all(&encoded)
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
            {
                state.pending.remove(&request_id);
                return Err(format!("failed writing live request: {error}"));
            }
        }

        match response_rx.recv_timeout(LIVE_REQUEST_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut state) = self.inner.state.lock() {
                    state.pending.remove(&request_id);
                }
                Err(format!("live request {request_id} timed out"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("live Workshop response channel closed".to_string())
            }
        }
    }

    pub(crate) fn stop(&self) -> Result<Value, String> {
        let request_id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        let encoded = serde_json::to_vec(&json!({
            "schema_version": 1,
            "request_id": request_id,
            "type": "quit"
        }))
        .map_err(|error| format!("failed serializing live stop request: {error}"))?;
        let (response_tx, response_rx) = mpsc::channel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| "live process state lock is poisoned".to_string())?;
            if state.stdin.is_none() {
                return Ok(json!({"state": "stopped"}));
            }
            state.pending.insert(request_id, response_tx);
            let stdin = state.stdin.as_mut().expect("checked live stdin");
            if let Err(error) = stdin
                .write_all(&encoded)
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
            {
                state.pending.remove(&request_id);
                return Err(format!("failed writing live stop request: {error}"));
            }
        }
        response_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "live Workshop did not acknowledge stop within 10 seconds".to_string())?
    }

    pub(crate) fn shutdown(&self) {
        let token = self
            .inner
            .state
            .lock()
            .map(|state| state.token)
            .unwrap_or(0);
        self.end_session(token, Some("Stasis language server stopped".to_string()));
    }

    fn read_stdout(&self, token: u64, stdout: impl std::io::Read) {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => {
                    self.end_session(token, None);
                    return;
                }
                Ok(_) if line.len() > MAX_PROTOCOL_LINE_BYTES => {
                    self.end_session(
                        token,
                        Some("live Workshop emitted a response larger than 16 MiB".to_string()),
                    );
                    return;
                }
                Ok(_) => match serde_json::from_slice::<Value>(&line) {
                    Ok(response) => self.accept_response(token, response),
                    Err(error) => {
                        let preview = String::from_utf8_lossy(&line);
                        let preview = preview.trim().chars().take(512).collect::<String>();
                        (self.inner.notify)(
                            LIVE_LOG_METHOD,
                            json!({
                                "stream": "stdout",
                                "message": format!("invalid live JSON: {error}; output={preview:?}")
                            }),
                        )
                    }
                },
                Err(error) => {
                    self.end_session(token, Some(format!("failed reading live stdout: {error}")));
                    return;
                }
            }
        }
    }

    fn read_stderr(&self, token: u64, stderr: impl std::io::Read) {
        for line in BufReader::new(stderr).lines() {
            let Ok(message) = line else {
                break;
            };
            let current = self
                .inner
                .state
                .lock()
                .is_ok_and(|state| state.token == token && state.child.is_some());
            if !current {
                return;
            }
            (self.inner.notify)(
                LIVE_LOG_METHOD,
                json!({"stream": "stderr", "message": message}),
            );
        }
    }

    fn accept_response(&self, token: u64, response: Value) {
        (self.inner.notify)(LIVE_EVENT_METHOD, response.clone());
        let request_id = response
            .get("request_id")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let kind = response
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let preparing = matches!(kind, "completion_preparing" | "edit_preparing");
        let mut pending = None;
        let mut cache_event = None;
        if let Ok(mut state) = self.inner.state.lock() {
            if state.token != token {
                return;
            }
            cache_event = state.cache.accept(&response);
            if request_id != 0 && !preparing {
                pending = state.pending.remove(&request_id);
            }
        }
        if let Some(event) = cache_event {
            self.publish_cache_event(event);
        }
        if let Some(pending) = pending {
            let _ = pending.send(Ok(response));
        }
    }

    fn end_session(&self, token: u64, error: Option<String>) {
        let pending = {
            let Ok(mut state) = self.inner.state.lock() else {
                return;
            };
            if state.token != token {
                return;
            }
            state.stdin.take();
            if let Some(mut child) = state.child.take() {
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            state.cache = ResponseCache::default();
            std::mem::take(&mut state.pending)
        };
        let detail = error.unwrap_or_else(|| "live Workshop stopped".to_string());
        for (_, pending) in pending {
            let _ = pending.send(Err(detail.clone()));
        }
        self.publish_cache_event(LiveCacheEvent::Clear);
        (self.inner.notify)(
            LIVE_STATE_METHOD,
            json!({"state": "stopped", "detail": detail}),
        );
    }

    fn publish_cache_event(&self, event: LiveCacheEvent) {
        if let Ok(mut mailbox) = self.inner.cache_event.lock() {
            *mailbox = Some(event);
        }
    }
}

impl Drop for BrokerInner {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut() {
            if let Some(mut child) = state.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl ResponseCache {
    fn accept(&mut self, response: &Value) -> Option<LiveCacheEvent> {
        if let Some(identity) = response
            .get("runtime_identity")
            .cloned()
            .and_then(|identity| serde_json::from_value(identity).ok())
        {
            self.identity = Some(identity);
        }
        let kind = response.get("kind").and_then(Value::as_str)?;
        let tick = response.get("tick").and_then(Value::as_u64).unwrap_or(0);
        let data = response.get("data")?;
        match kind {
            "inspection" | "watch" | "watch_added" => {
                let observation_source = data.get("inspection").unwrap_or(data);
                if let Some(observation) = observation_from_value(observation_source, tick) {
                    self.record_observation(observation);
                }
            }
            "state_inspection" => {
                if let Some(items) = data.get("items").and_then(Value::as_array) {
                    for item in items {
                        if let Some(observation) = observation_from_value(item, tick) {
                            self.record_observation(observation);
                        }
                    }
                }
            }
            _ => {}
        }
        let identity = self.identity.as_ref()?;
        Some(LiveCacheEvent::Publish(LiveObservationBatch {
            session_id: identity.session_id.clone(),
            generation: identity.generation,
            source_hashes: identity.source_hashes.clone(),
            indexed_collections: identity
                .indexed_collections
                .iter()
                .map(|collection| LiveIndexedCollection {
                    path: collection.path.clone(),
                    fields: collection.fields.clone(),
                })
                .collect(),
            complete: identity.complete,
            observations: self.observations.values().take(512).cloned().collect(),
        }))
    }

    fn record_observation(&mut self, mut observation: LiveObservation) {
        if observation.type_name.is_none() {
            observation.type_name = self
                .observations
                .get(&observation.path)
                .and_then(|previous| previous.type_name.clone());
        }
        if observation.type_name.is_some() {
            self.observations
                .insert(observation.path.clone(), observation);
        }
    }
}

fn observation_from_value(value: &Value, tick: u64) -> Option<LiveObservation> {
    let path = value.get("path")?.as_str()?.to_string();
    let type_name = value
        .get("static_type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let value = display_value(value.get("value")?);
    Some(LiveObservation {
        path,
        type_name,
        value,
        tick,
    })
}

fn display_value(value: &Value) -> String {
    let value = value.get("value").unwrap_or(value);
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "<value>".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_cache_composes_runtime_identity_and_scalar_observations() {
        let mut cache = ResponseCache::default();
        let event = cache
            .accept(&json!({
                "request_id": 2,
                "tick": 9,
                "kind": "inspection",
                "runtime_identity": {
                    "session_id": "session-1",
                    "generation": 3,
                    "source_hashes": {"src/main.stasis": "abc"},
                    "indexed_collections": [{
                        "path": "state.enemies",
                        "fields": {"speed": "f32"}
                    }],
                    "complete": true
                },
                "data": {
                    "path": "score",
                    "static_type": "i32",
                    "value": {"type": "i32", "value": 12}
                }
            }))
            .expect("cache event");
        let LiveCacheEvent::Publish(batch) = event else {
            panic!("expected publication");
        };
        assert_eq!(batch.session_id, "session-1");
        assert_eq!(batch.generation, 3);
        assert!(batch.complete);
        assert_eq!(batch.indexed_collections[0].path, "state.enemies");
        assert_eq!(
            batch.observations,
            vec![LiveObservation {
                path: "score".to_string(),
                type_name: Some("i32".to_string()),
                value: "12".to_string(),
                tick: 9,
            }]
        );

        let LiveCacheEvent::Publish(batch) = cache
            .accept(&json!({
                "request_id": 0,
                "tick": 10,
                "kind": "watch",
                "data": {"path": "score", "value": 13}
            }))
            .expect("watch cache event")
        else {
            panic!("expected watch publication");
        };
        assert_eq!(batch.observations[0].type_name.as_deref(), Some("i32"));
        assert_eq!(batch.observations[0].value, "13");
        assert_eq!(batch.observations[0].tick, 10);
    }
}
