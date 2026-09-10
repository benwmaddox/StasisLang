//! Durable, project-scoped editor session state.
//!
//! Task-owned media is deliberately retained by [`SessionStore::erase`]. Media removal belongs to
//! the attachment and generated-image lifecycle, where provenance and handoff state are available.

use crate::task_session::{
    ScreenshotAnalysisState, TaskSession, UploadState, MAX_ACTIONS, MAX_ACTION_REVISIONS,
    MAX_IMAGES, MAX_SCREENSHOTS, MAX_TASKS, MAX_THREAD_ENTRIES,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const SCHEMA_NAME: &str = "stasis-ai-editor-session";
const CURRENT_VERSION: u32 = 2;
const STATE_FILE: &str = "session.json";
const PENDING_FILE: &str = "session.pending";
const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECEIPTS: usize = 2_048;
const MAX_FINGERPRINTS: usize = 2_048;
const MAX_EXPANDED: usize = 4_096;
const MAX_MEDIA: usize = MAX_TASKS * (MAX_SCREENSHOTS + MAX_IMAGES);
const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionSnapshot {
    pub session: TaskSession,
    pub task_order: Vec<String>,
    pub drafts: BTreeMap<String, (String, String)>,
    pub objective: String,
    pub reply: String,
    pub next_task_number: u64,
    pub next_capture_number: u64,
    pub execution_receipts: Vec<ExecutionReceipt>,
    pub validation_receipts: BTreeMap<String, Value>,
    pub validation_fingerprints: BTreeMap<String, (String, Vec<String>)>,
    pub preview_fingerprints: BTreeMap<String, (String, bool)>,
    pub in_flight: BTreeSet<String>,
    pub uncertain_calls: BTreeSet<String>,
    pub expanded: BTreeSet<String>,
    pub window_preferences: Option<WindowPreferences>,
    /// Expected SHA-256 values keyed by the exact media source stored on the task.
    pub media_hashes: BTreeMap<String, String>,
    /// Media source paths that could not be verified during the latest load.
    pub unavailable_media: BTreeSet<String>,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            session: TaskSession::default(),
            task_order: Vec::new(),
            drafts: BTreeMap::new(),
            objective: String::new(),
            reply: String::new(),
            next_task_number: 1,
            next_capture_number: 1,
            execution_receipts: Vec::new(),
            validation_receipts: BTreeMap::new(),
            validation_fingerprints: BTreeMap::new(),
            preview_fingerprints: BTreeMap::new(),
            in_flight: BTreeSet::new(),
            uncertain_calls: BTreeSet::new(),
            expanded: BTreeSet::new(),
            window_preferences: None,
            media_hashes: BTreeMap::new(),
            unavailable_media: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReceipt {
    pub task_id: String,
    pub action_id: String,
    pub receipt: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowPreferences {
    pub size: [f32; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDiagnostic {
    Migrated { from: u32, to: u32 },
    InterruptedWriteDiscarded,
    UncertainProviderCall { task_id: String },
    MediaMissing { source: String },
    MediaChanged { source: String },
    MediaUnverified { source: String },
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Corrupt { path: PathBuf, message: String },
    UnsupportedVersion(u32),
    ProjectMismatch { expected: String, found: String },
    LimitExceeded { field: &'static str, limit: usize },
    PrivacyViolation { field: String },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Corrupt { path, message } => {
                write!(f, "corrupt editor session {}: {message}", path.display())
            }
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported editor session schema version {version}")
            }
            Self::ProjectMismatch { expected, found } => write!(
                f,
                "editor session belongs to project {found}, expected {expected}"
            ),
            Self::LimitExceeded { field, limit } => {
                write!(f, "persisted {field} exceeds limit {limit}")
            }
            Self::PrivacyViolation { field } => {
                write!(f, "refusing to persist private provider field: {field}")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadOutcome {
    pub snapshot: Option<SessionSnapshot>,
    pub diagnostics: Vec<RecoveryDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDocument {
    schema: String,
    version: u32,
    project: String,
    snapshot: Value,
}

#[derive(Debug, Serialize)]
struct StoredDocumentRef<'a> {
    schema: &'static str,
    version: u32,
    project: &'a str,
    snapshot: &'a SessionSnapshot,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SnapshotV1 {
    session: TaskSession,
    task_order: Vec<String>,
    drafts: BTreeMap<String, (String, String)>,
    objective: String,
    reply: String,
    next_task_number: u64,
    next_capture_number: u64,
    execution_receipts: Vec<ExecutionReceipt>,
    validation_receipts: BTreeMap<String, Value>,
    validation_fingerprints: BTreeMap<String, (String, Vec<String>)>,
    in_flight: BTreeSet<String>,
    expanded: BTreeSet<String>,
    window_preferences: Option<WindowPreferences>,
}

impl From<SnapshotV1> for SessionSnapshot {
    fn from(old: SnapshotV1) -> Self {
        Self {
            session: old.session,
            task_order: old.task_order,
            drafts: old.drafts,
            objective: old.objective,
            reply: old.reply,
            next_task_number: old.next_task_number.max(1),
            next_capture_number: old.next_capture_number.max(1),
            execution_receipts: old.execution_receipts,
            validation_receipts: old.validation_receipts,
            validation_fingerprints: old.validation_fingerprints,
            in_flight: old.in_flight,
            expanded: old.expanded,
            window_preferences: old.window_preferences,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    project_root: PathBuf,
    project_identity: String,
    state_dir: PathBuf,
}

impl SessionStore {
    pub fn open(project_root: impl AsRef<Path>) -> io::Result<Self> {
        let project_root = fs::canonicalize(project_root)?;
        let project_identity = canonical_identity(&project_root);
        let state_dir = project_root.join(".stasis").join("editor");
        Ok(Self {
            project_root,
            project_identity,
            state_dir,
        })
    }

    pub fn load(&self) -> Result<LoadOutcome, StoreError> {
        let state_path = self.state_dir.join(STATE_FILE);
        let pending_path = self.state_dir.join(PENDING_FILE);
        let interrupted = pending_path.exists();
        if !state_path.exists() {
            return Ok(LoadOutcome {
                snapshot: None,
                diagnostics: interrupted
                    .then_some(RecoveryDiagnostic::InterruptedWriteDiscarded)
                    .into_iter()
                    .collect(),
            });
        }

        let metadata = fs::metadata(&state_path)?;
        if metadata.len() > MAX_STATE_BYTES {
            return Err(StoreError::LimitExceeded {
                field: "session bytes",
                limit: MAX_STATE_BYTES as usize,
            });
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(&state_path)?
            .take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(StoreError::LimitExceeded {
                field: "session bytes",
                limit: MAX_STATE_BYTES as usize,
            });
        }
        let document: StoredDocument =
            serde_json::from_slice(&bytes).map_err(|error| StoreError::Corrupt {
                path: state_path.clone(),
                message: error.to_string(),
            })?;
        if document.schema != SCHEMA_NAME {
            return Err(StoreError::Corrupt {
                path: state_path,
                message: format!("unexpected schema {:?}", document.schema),
            });
        }
        if document.project != self.project_identity {
            return Err(StoreError::ProjectMismatch {
                expected: self.project_identity.clone(),
                found: document.project,
            });
        }

        let mut diagnostics = Vec::new();
        if interrupted {
            diagnostics.push(RecoveryDiagnostic::InterruptedWriteDiscarded);
        }
        let mut snapshot = match document.version {
            1 => {
                diagnostics.push(RecoveryDiagnostic::Migrated {
                    from: 1,
                    to: CURRENT_VERSION,
                });
                serde_json::from_value::<SnapshotV1>(document.snapshot).map(SessionSnapshot::from)
            }
            CURRENT_VERSION => serde_json::from_value(document.snapshot),
            version => return Err(StoreError::UnsupportedVersion(version)),
        }
        .map_err(|error| StoreError::Corrupt {
            path: self.state_dir.join(STATE_FILE),
            message: error.to_string(),
        })?;
        validate_snapshot(&snapshot)?;
        validate_privacy(&snapshot)?;

        for task_id in std::mem::take(&mut snapshot.in_flight) {
            snapshot.uncertain_calls.insert(task_id.clone());
            diagnostics.push(RecoveryDiagnostic::UncertainProviderCall { task_id });
        }
        self.revalidate_media(&mut snapshot, &mut diagnostics);
        Ok(LoadOutcome {
            snapshot: Some(snapshot),
            diagnostics,
        })
    }

    pub fn save(&self, snapshot: &SessionSnapshot) -> Result<(), StoreError> {
        let mut persisted = snapshot.clone();
        self.prepare_snapshot(&mut persisted)?;
        let document = StoredDocumentRef {
            schema: SCHEMA_NAME,
            version: CURRENT_VERSION,
            project: &self.project_identity,
            snapshot: &persisted,
        };
        let value = serde_json::to_value(&document).map_err(|error| StoreError::Corrupt {
            path: self.state_dir.join(STATE_FILE),
            message: error.to_string(),
        })?;
        validate_privacy(&persisted)?;
        let mut bytes = serde_json::to_vec_pretty(&value).map_err(|error| StoreError::Corrupt {
            path: self.state_dir.join(STATE_FILE),
            message: error.to_string(),
        })?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(StoreError::LimitExceeded {
                field: "session bytes",
                limit: MAX_STATE_BYTES as usize,
            });
        }

        fs::create_dir_all(&self.state_dir)?;
        write_pending_marker(&self.state_dir.join(PENDING_FILE))?;
        let state_path = self.state_dir.join(STATE_FILE);
        let write_result = (|| -> Result<(), StoreError> {
            let mut file = atomic_write_file::AtomicWriteFile::open(&state_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            file.commit()?;
            sync_directory(&self.state_dir)?;
            Ok(())
        })();
        if write_result.is_ok() {
            remove_if_exists(&self.state_dir.join(PENDING_FILE))?;
        }
        write_result
    }

    /// Captures stable media fingerprints in editor-owned state before it is saved.
    ///
    /// Callers keep this prepared snapshot so later saves do not silently accept changed media as
    /// a new baseline.
    pub fn prepare_snapshot(&self, snapshot: &mut SessionSnapshot) -> Result<(), StoreError> {
        self.capture_media_hashes(snapshot);
        validate_snapshot(snapshot)
    }

    pub fn erase(&self) -> Result<(), StoreError> {
        remove_if_exists(&self.state_dir.join(STATE_FILE))?;
        remove_if_exists(&self.state_dir.join(PENDING_FILE))?;
        self.erase_atomic_temporary_files()?;
        sync_directory(&self.state_dir)?;
        Ok(())
    }

    fn erase_atomic_temporary_files(&self) -> Result<(), StoreError> {
        let entries = match fs::read_dir(&self.state_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if is_session_atomic_temporary_file(&entry.file_name()) {
                remove_if_exists(&entry.path())?;
            }
        }
        Ok(())
    }

    fn capture_media_hashes(&self, snapshot: &mut SessionSnapshot) {
        if snapshot.media_hashes.len() >= MAX_MEDIA {
            return;
        }
        let sources = media_sources(snapshot);
        for source in sources {
            if snapshot.media_hashes.len() >= MAX_MEDIA
                || snapshot.media_hashes.contains_key(&source)
                || snapshot.unavailable_media.contains(&source)
            {
                continue;
            }
            let attachment_hash = screenshot_hash(snapshot, &source);
            if let Some(hash) =
                attachment_hash.or_else(|| hash_media(&self.resolve_source(&source)).ok())
            {
                snapshot.media_hashes.insert(source, hash);
            }
        }
    }

    fn revalidate_media(
        &self,
        snapshot: &mut SessionSnapshot,
        diagnostics: &mut Vec<RecoveryDiagnostic>,
    ) {
        snapshot.unavailable_media.clear();
        for source in media_sources(snapshot) {
            let expected_hash = snapshot
                .media_hashes
                .get(&source)
                .cloned()
                .or_else(|| screenshot_hash(snapshot, &source));
            let diagnostic = match (hash_media(&self.resolve_source(&source)), expected_hash) {
                (Ok(actual), Some(expected)) if actual != expected => {
                    RecoveryDiagnostic::MediaChanged {
                        source: source.clone(),
                    }
                }
                (Ok(_), Some(_)) => continue,
                (Ok(_), None) => RecoveryDiagnostic::MediaUnverified {
                    source: source.clone(),
                },
                (Err(_), _) => RecoveryDiagnostic::MediaMissing {
                    source: source.clone(),
                },
            };
            snapshot.unavailable_media.insert(source.clone());
            for task in snapshot.session.tasks.values_mut() {
                for screenshot in task.screenshots.values_mut() {
                    if screenshot.source == source {
                        let reason = match diagnostic {
                            RecoveryDiagnostic::MediaChanged { .. } => {
                                "attachment changed after it was saved"
                            }
                            RecoveryDiagnostic::MediaUnverified { .. } => {
                                "attachment cannot be verified after restart"
                            }
                            _ => "attachment is missing after restart",
                        };
                        screenshot.upload = UploadState::Failed {
                            reason: reason.to_string(),
                        };
                        screenshot.analysis = ScreenshotAnalysisState::Failed {
                            reason: reason.to_string(),
                        };
                    }
                }
            }
            diagnostics.push(diagnostic);
        }
    }

    fn resolve_source(&self, source: &str) -> PathBuf {
        let path = Path::new(source);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        }
    }
}

fn validate_snapshot(snapshot: &SessionSnapshot) -> Result<(), StoreError> {
    let corrupt_identity = || StoreError::Corrupt {
        path: PathBuf::from(STATE_FILE),
        message: "saved task identities are inconsistent".into(),
    };
    if snapshot
        .session
        .active_task_id
        .as_ref()
        .is_some_and(|id| !snapshot.session.tasks.contains_key(id))
    {
        return Err(corrupt_identity());
    }
    let queued_ids = snapshot
        .session
        .queued_task_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if queued_ids.len() != snapshot.session.queued_task_ids.len()
        || snapshot.session.queued_task_ids.iter().any(|id| {
            !snapshot.session.tasks.contains_key(id)
                || snapshot
                    .session
                    .tasks
                    .get(id)
                    .is_some_and(|task| task.lifecycle != crate::TaskLifecycle::Queued)
        })
    {
        return Err(corrupt_identity());
    }
    for (id, task) in &snapshot.session.tasks {
        if id != &task.id
            || task.actions.iter().any(|(id, action)| id != &action.id)
            || task.screenshots.iter().any(|(id, media)| id != &media.id)
            || task
                .generated_images
                .iter()
                .any(|(id, media)| id != &media.id)
        {
            return Err(corrupt_identity());
        }
    }
    check_limit("tasks", snapshot.session.tasks.len(), MAX_TASKS)?;
    check_limit(
        "queued tasks",
        snapshot.session.queued_task_ids.len(),
        MAX_TASKS,
    )?;
    check_limit("task order", snapshot.task_order.len(), MAX_TASKS)?;
    check_limit("drafts", snapshot.drafts.len(), MAX_TASKS)?;
    check_limit(
        "execution receipts",
        snapshot.execution_receipts.len(),
        MAX_RECEIPTS,
    )?;
    check_limit(
        "validation receipts",
        snapshot.validation_receipts.len(),
        MAX_RECEIPTS,
    )?;
    check_limit(
        "validation fingerprints",
        snapshot.validation_fingerprints.len(),
        MAX_FINGERPRINTS,
    )?;
    check_limit("expanded rows", snapshot.expanded.len(), MAX_EXPANDED)?;
    check_limit(
        "preview fingerprints",
        snapshot.preview_fingerprints.len(),
        MAX_EXPANDED,
    )?;
    check_limit("media hashes", snapshot.media_hashes.len(), MAX_MEDIA)?;
    check_limit("in-flight calls", snapshot.in_flight.len(), MAX_TASKS)?;
    check_limit("uncertain calls", snapshot.uncertain_calls.len(), MAX_TASKS)?;
    check_limit(
        "unavailable media",
        snapshot.unavailable_media.len(),
        MAX_MEDIA,
    )?;
    check_limit("media sources", media_sources(snapshot).len(), MAX_MEDIA)?;
    for (_, attachment_hashes) in snapshot.validation_fingerprints.values() {
        check_limit("fingerprint paths", attachment_hashes.len(), MAX_MEDIA)?;
    }
    for hash in snapshot.media_hashes.values() {
        if !is_sha256(hash) {
            return Err(StoreError::Corrupt {
                path: PathBuf::from(STATE_FILE),
                message: "media fingerprint is not a lowercase SHA-256 value".to_string(),
            });
        }
    }
    for task in snapshot.session.tasks.values() {
        check_limit("thread entries", task.thread.len(), MAX_THREAD_ENTRIES)?;
        check_limit("actions", task.actions.len(), MAX_ACTIONS)?;
        check_limit("screenshots", task.screenshots.len(), MAX_SCREENSHOTS)?;
        check_limit("generated images", task.generated_images.len(), MAX_IMAGES)?;
        check_limit(
            "activity entries",
            task.activity.len(),
            MAX_THREAD_ENTRIES + MAX_ACTIONS + MAX_SCREENSHOTS + MAX_IMAGES,
        )?;
        for screenshot in task.screenshots.values() {
            if screenshot
                .content_sha256
                .as_deref()
                .is_some_and(|hash| !is_sha256(hash))
            {
                return Err(StoreError::Corrupt {
                    path: PathBuf::from(STATE_FILE),
                    message: "screenshot fingerprint is not a lowercase SHA-256 value".to_string(),
                });
            }
        }
        for action in task.actions.values() {
            check_limit(
                "action revisions",
                action.revisions.len(),
                MAX_ACTION_REVISIONS,
            )?;
        }
    }
    if let Some(preferences) = snapshot.window_preferences {
        if preferences
            .size
            .iter()
            .any(|value| !value.is_finite() || *value < 320.0 || *value > 16_384.0)
        {
            return Err(StoreError::Corrupt {
                path: PathBuf::from(STATE_FILE),
                message: "window size is outside safe bounds".to_string(),
            });
        }
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_session_atomic_temporary_file(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let prefix = format!(".{STATE_FILE}.");
    let Some(suffix) = name.strip_prefix(&prefix) else {
        return false;
    };
    suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn check_limit(field: &'static str, actual: usize, limit: usize) -> Result<(), StoreError> {
    if actual > limit {
        Err(StoreError::LimitExceeded { field, limit })
    } else {
        Ok(())
    }
}

fn media_sources(snapshot: &SessionSnapshot) -> BTreeSet<String> {
    snapshot
        .session
        .tasks
        .values()
        .flat_map(|task| {
            task.screenshots
                .values()
                .map(|item| item.source.clone())
                .chain(
                    task.generated_images
                        .values()
                        .map(|item| item.source.clone()),
                )
        })
        .collect()
}

fn screenshot_hash(snapshot: &SessionSnapshot, source: &str) -> Option<String> {
    snapshot.session.tasks.values().find_map(|task| {
        task.screenshots
            .values()
            .filter(|screenshot| screenshot.source == source)
            .find_map(|screenshot| screenshot.content_sha256.clone())
    })
}

fn hash_media(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() > MAX_MEDIA_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "media exceeds persisted-session hash limit",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_MEDIA_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "media exceeds persisted-session hash limit",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_privacy(snapshot: &SessionSnapshot) -> Result<(), StoreError> {
    // IDs and semantic payloads are user data, not provider envelope field names.
    for receipt in &snapshot.execution_receipts {
        reject_private_fields(&receipt.receipt)?;
    }
    for receipt in snapshot.validation_receipts.values() {
        reject_private_fields(receipt)?;
    }
    Ok(())
}

fn reject_private_fields(value: &Value) -> Result<(), StoreError> {
    if let Value::Object(object) = value {
        for key in object.keys() {
            let normalized = key
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if matches!(
                normalized.as_str(),
                "authorization"
                    | "authorizationheaders"
                    | "apikey"
                    | "accesstoken"
                    | "refreshtoken"
                    | "clientsecret"
                    | "password"
                    | "credential"
                    | "credentials"
                    | "storedcredentials"
                    | "rawproviderenvelope"
                    | "rawenvelope"
                    | "providerrawenvelope"
                    | "providerrawenvelopes"
                    | "hiddenreasoning"
                    | "modelhiddenreasoning"
                    | "chainofthought"
                    | "workingnotes"
            ) {
                return Err(StoreError::PrivacyViolation { field: key.clone() });
            }
        }
    }
    Ok(())
}

fn write_pending_marker(path: &Path) -> io::Result<()> {
    let mut marker = File::create(path)?;
    marker.write_all(b"pending schema 2\n")?;
    marker.sync_all()
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn canonical_identity(path: &Path) -> String {
    let identity = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        identity.to_lowercase()
    } else {
        identity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScreenshotId, TaskId, TaskProvenance, VisionCapability};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestProject(PathBuf);

    impl TestProject {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "stasis-ai-store-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test project");
            Self(path)
        }

        fn store(&self) -> SessionStore {
            SessionStore::open(&self.0).expect("open store")
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn round_trip_preserves_editor_state() {
        let project = TestProject::new("round-trip");
        let store = project.store();
        let mut snapshot = SessionSnapshot {
            objective: "Build a game".into(),
            reply: "Use the accepted revision".into(),
            next_task_number: 8,
            next_capture_number: 3,
            task_order: vec!["task-1".into()],
            window_preferences: Some(WindowPreferences {
                size: [1280.0, 720.0],
            }),
            ..SessionSnapshot::default()
        };
        snapshot
            .session
            .new_task("task-1", "Fix movement", "Sample")
            .expect("task");
        snapshot
            .drafts
            .insert("task-1".into(), ("objective".into(), "reply".into()));
        snapshot.expanded.insert("task-1/action-1".into());
        snapshot.validation_fingerprints.insert(
            "task-1".into(),
            ("source hash".into(), vec!["attachment hash".into()]),
        );
        snapshot.execution_receipts.push(ExecutionReceipt {
            task_id: "task-1".into(),
            action_id: "action-1".into(),
            receipt: serde_json::json!({"passed": true}),
        });
        fs::write(project.0.join("generated.png"), b"generated pixels").unwrap();
        let task = snapshot.session.active_task_mut().unwrap();
        task.add_generated_image(
            "image-1",
            "generated.png",
            crate::task_session::ImageAttribution::new("fixture", None, None).unwrap(),
        )
        .unwrap();
        task.approve_generated_image("image-1").unwrap();
        store.prepare_snapshot(&mut snapshot).unwrap();

        store.save(&snapshot).expect("save");
        let loaded = store.load().expect("load");
        assert_eq!(loaded.snapshot, Some(snapshot));
        assert!(loaded.diagnostics.is_empty());
    }

    #[test]
    fn migrates_v1_and_supplies_new_fields() {
        let project = TestProject::new("migration");
        let store = project.store();
        fs::create_dir_all(&store.state_dir).unwrap();
        let document = serde_json::json!({
            "schema": SCHEMA_NAME,
            "version": 1,
            "project": store.project_identity,
            "snapshot": {"objective": "old draft", "in_flight": ["task-4"]}
        });
        fs::write(
            store.state_dir.join(STATE_FILE),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();

        let loaded = store.load().expect("migrate");
        let snapshot = loaded.snapshot.unwrap();
        assert_eq!(snapshot.objective, "old draft");
        assert_eq!(snapshot.next_task_number, 1);
        assert!(snapshot.in_flight.is_empty());
        assert!(snapshot.uncertain_calls.contains("task-4"));
        assert!(loaded
            .diagnostics
            .contains(&RecoveryDiagnostic::Migrated { from: 1, to: 2 }));
    }

    #[test]
    fn pending_marker_reports_interrupted_atomic_write_and_keeps_state() {
        let project = TestProject::new("interrupted");
        let store = project.store();
        let snapshot = SessionSnapshot {
            objective: "complete state".into(),
            ..SessionSnapshot::default()
        };
        store.save(&snapshot).unwrap();
        write_pending_marker(&store.state_dir.join(PENDING_FILE)).unwrap();
        fs::write(store.state_dir.join("abandoned.tmp"), b"{truncated").unwrap();

        let loaded = store.load().expect("old atomic state remains valid");
        assert_eq!(loaded.snapshot.unwrap().objective, "complete state");
        assert!(loaded
            .diagnostics
            .contains(&RecoveryDiagnostic::InterruptedWriteDiscarded));
    }

    #[test]
    fn uncommitted_atomic_write_reports_interruption_and_keeps_state() {
        let project = TestProject::new("atomic-interruption");
        let store = project.store();
        store
            .save(&SessionSnapshot {
                objective: "last committed state".into(),
                ..SessionSnapshot::default()
            })
            .unwrap();

        write_pending_marker(&store.state_dir.join(PENDING_FILE)).unwrap();
        let mut interrupted =
            atomic_write_file::AtomicWriteFile::open(store.state_dir.join(STATE_FILE)).unwrap();
        interrupted.write_all(b"{truncated replacement").unwrap();
        interrupted.sync_all().unwrap();
        drop(interrupted);

        let loaded = store.load().expect("committed state survives");
        assert_eq!(loaded.snapshot.unwrap().objective, "last committed state");
        assert!(loaded
            .diagnostics
            .contains(&RecoveryDiagnostic::InterruptedWriteDiscarded));
    }

    #[test]
    fn truncated_state_has_a_specific_corruption_error() {
        let project = TestProject::new("corrupt");
        let store = project.store();
        fs::create_dir_all(&store.state_dir).unwrap();
        fs::write(store.state_dir.join(STATE_FILE), b"{\"schema\":").unwrap();
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
    }

    #[test]
    fn unsupported_version_preserves_the_original_document() {
        let project = TestProject::new("future-version");
        let store = project.store();
        store.save(&SessionSnapshot::default()).unwrap();
        let path = store.state_dir.join(STATE_FILE);
        let mut document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        document["version"] = Value::from(CURRENT_VERSION + 1);
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(&path, &bytes).unwrap();

        assert!(matches!(
            store.load(),
            Err(StoreError::UnsupportedVersion(3))
        ));
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn malformed_task_identities_are_reported_as_corruption() {
        let project = TestProject::new("corrupt-identities");
        let store = project.store();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("task-1", "Review", "Sample")
            .unwrap();
        store.save(&snapshot).unwrap();
        let path = store.state_dir.join(STATE_FILE);
        let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for field in ["active", "embedded"] {
            let mut document = original.clone();
            if field == "active" {
                document["snapshot"]["session"]["active_task_id"] = Value::from("missing");
            } else {
                document["snapshot"]["session"]["tasks"]["task-1"]["id"] = Value::from("other");
            }
            fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
            assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
        }
    }

    #[test]
    fn byte_limit_preserves_committed_state_and_bounds_loading() {
        let project = TestProject::new("byte-limit");
        let store = project.store();
        let committed = SessionSnapshot {
            reply: "Last committed draft".into(),
            ..SessionSnapshot::default()
        };
        store.save(&committed).unwrap();
        let oversized = SessionSnapshot {
            reply: "x".repeat(MAX_STATE_BYTES as usize),
            ..SessionSnapshot::default()
        };
        assert!(matches!(
            store.save(&oversized),
            Err(StoreError::LimitExceeded {
                field: "session bytes",
                ..
            })
        ));
        assert_eq!(store.load().unwrap().snapshot, Some(committed));

        File::create(store.state_dir.join(STATE_FILE))
            .unwrap()
            .set_len(MAX_STATE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            store.load(),
            Err(StoreError::LimitExceeded {
                field: "session bytes",
                ..
            })
        ));
    }

    #[test]
    fn copied_state_cannot_cross_project_identity() {
        let first = TestProject::new("isolation-a");
        let second = TestProject::new("isolation-b");
        let first_store = first.store();
        let second_store = second.store();
        first_store.save(&SessionSnapshot::default()).unwrap();
        fs::create_dir_all(&second_store.state_dir).unwrap();
        fs::copy(
            first_store.state_dir.join(STATE_FILE),
            second_store.state_dir.join(STATE_FILE),
        )
        .unwrap();
        assert!(matches!(
            second_store.load(),
            Err(StoreError::ProjectMismatch { .. })
        ));
    }

    #[test]
    fn rejects_state_beyond_retention_bounds() {
        let project = TestProject::new("bounds");
        let store = project.store();
        let committed = SessionSnapshot {
            reply: "Keep this saved draft".into(),
            ..SessionSnapshot::default()
        };
        store.save(&committed).unwrap();
        let snapshot = SessionSnapshot {
            task_order: (0..=MAX_TASKS)
                .map(|index| format!("task-{index}"))
                .collect(),
            ..SessionSnapshot::default()
        };
        assert!(matches!(
            store.save(&snapshot),
            Err(StoreError::LimitExceeded {
                field: "task order",
                ..
            })
        ));
        assert_eq!(store.load().unwrap().snapshot, Some(committed));
    }

    #[test]
    fn restart_marks_missing_and_changed_media_unavailable() {
        let project = TestProject::new("media");
        let store = project.store();
        let media = project.0.join("capture.png");
        fs::write(&media, b"original pixels").unwrap();
        let hash = hash_media(&media).unwrap();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("task-1", "Review capture", "Sample")
            .unwrap();
        let task = snapshot.session.active_task_mut().unwrap();
        task.vision_capability = VisionCapability::Available;
        task.screenshots.insert(
            ScreenshotId::new("capture-1"),
            crate::ScreenshotAttachment {
                request_id: None,
                selected_for_request: false,
                consent_to_send: false,
                id: ScreenshotId::new("capture-1"),
                source: "capture.png".into(),
                content_sha256: Some(hash),
                provenance: TaskProvenance {
                    task_id: TaskId::new("task-1"),
                },
                vision: VisionCapability::Available,
                upload: UploadState::Uploaded,
                analysis: ScreenshotAnalysisState::Completed,
            },
        );
        store.save(&snapshot).unwrap();

        fs::write(&media, b"different pixels").unwrap();
        let changed = store.load().unwrap();
        assert!(changed
            .diagnostics
            .contains(&RecoveryDiagnostic::MediaChanged {
                source: "capture.png".into()
            }));
        let changed_snapshot = changed.snapshot.unwrap();
        assert!(changed_snapshot.unavailable_media.contains("capture.png"));
        assert!(matches!(
            changed_snapshot.session.tasks["task-1"].screenshots["capture-1"].upload,
            UploadState::Failed { .. }
        ));

        fs::remove_file(media).unwrap();
        let missing = store.load().unwrap();
        assert!(missing
            .diagnostics
            .contains(&RecoveryDiagnostic::MediaMissing {
                source: "capture.png".into()
            }));
    }

    #[test]
    fn restart_marks_missing_media_without_a_prior_hash() {
        let project = TestProject::new("unhashed-missing-media");
        let store = project.store();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("task-1", "Review capture", "Sample")
            .unwrap();
        let task = snapshot.session.active_task_mut().unwrap();
        task.vision_capability = VisionCapability::Available;
        task.screenshots.insert(
            ScreenshotId::new("capture-1"),
            crate::ScreenshotAttachment {
                request_id: None,
                selected_for_request: false,
                consent_to_send: false,
                id: ScreenshotId::new("capture-1"),
                source: "never-created.png".into(),
                content_sha256: None,
                provenance: TaskProvenance {
                    task_id: TaskId::new("task-1"),
                },
                vision: VisionCapability::Available,
                upload: UploadState::Uploaded,
                analysis: ScreenshotAnalysisState::Completed,
            },
        );

        store.save(&snapshot).unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded
            .diagnostics
            .contains(&RecoveryDiagnostic::MediaMissing {
                source: "never-created.png".into()
            }));
        assert!(loaded
            .snapshot
            .unwrap()
            .unavailable_media
            .contains("never-created.png"));
    }

    #[test]
    fn restart_quarantines_existing_media_without_a_prior_hash() {
        let project = TestProject::new("unverified-media");
        let store = project.store();
        fs::write(project.0.join("legacy.png"), b"unverified pixels").unwrap();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("task-1", "Review capture", "Sample")
            .unwrap();
        let task = snapshot.session.active_task_mut().unwrap();
        task.vision_capability = VisionCapability::Available;
        task.screenshots.insert(
            ScreenshotId::new("capture-1"),
            crate::ScreenshotAttachment {
                request_id: None,
                selected_for_request: false,
                consent_to_send: false,
                id: ScreenshotId::new("capture-1"),
                source: "legacy.png".into(),
                content_sha256: None,
                provenance: TaskProvenance {
                    task_id: TaskId::new("task-1"),
                },
                vision: VisionCapability::Available,
                upload: UploadState::Uploaded,
                analysis: ScreenshotAnalysisState::Completed,
            },
        );
        fs::create_dir_all(&store.state_dir).unwrap();
        let document = StoredDocumentRef {
            schema: SCHEMA_NAME,
            version: CURRENT_VERSION,
            project: &store.project_identity,
            snapshot: &snapshot,
        };
        fs::write(
            store.state_dir.join(STATE_FILE),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded
            .diagnostics
            .contains(&RecoveryDiagnostic::MediaUnverified {
                source: "legacy.png".into()
            }));
        let mut recovered = loaded.snapshot.unwrap();
        assert!(recovered.unavailable_media.contains("legacy.png"));
        store.prepare_snapshot(&mut recovered).unwrap();
        assert!(!recovered.media_hashes.contains_key("legacy.png"));
    }

    #[test]
    fn prepared_media_hash_remains_the_baseline_across_saves() {
        let project = TestProject::new("stable-media-hash");
        let store = project.store();
        let media = project.0.join("capture.png");
        fs::write(&media, b"original pixels").unwrap();
        let original_hash = hash_media(&media).unwrap();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("task-1", "Review capture", "Sample")
            .unwrap();
        let task = snapshot.session.active_task_mut().unwrap();
        task.vision_capability = VisionCapability::Available;
        task.screenshots.insert(
            ScreenshotId::new("capture-1"),
            crate::ScreenshotAttachment {
                request_id: None,
                selected_for_request: false,
                consent_to_send: false,
                id: ScreenshotId::new("capture-1"),
                source: "capture.png".into(),
                content_sha256: None,
                provenance: TaskProvenance {
                    task_id: TaskId::new("task-1"),
                },
                vision: VisionCapability::Available,
                upload: UploadState::Uploaded,
                analysis: ScreenshotAnalysisState::Completed,
            },
        );

        store.prepare_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.media_hashes["capture.png"], original_hash);
        store.save(&snapshot).unwrap();
        fs::write(&media, b"changed pixels").unwrap();
        store.save(&snapshot).unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded
            .diagnostics
            .contains(&RecoveryDiagnostic::MediaChanged {
                source: "capture.png".into()
            }));
    }

    #[test]
    fn preparation_prefers_attachment_hash_over_current_disk_bytes() {
        let project = TestProject::new("attachment-hash");
        let store = project.store();
        let media = project.0.join("capture.png");
        fs::write(&media, b"captured pixels").unwrap();
        let captured_hash = hash_media(&media).unwrap();
        fs::write(&media, b"changed before save").unwrap();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("task-1", "Review capture", "Sample")
            .unwrap();
        let task = snapshot.session.active_task_mut().unwrap();
        task.vision_capability = VisionCapability::Available;
        task.screenshots.insert(
            ScreenshotId::new("capture-1"),
            crate::ScreenshotAttachment {
                request_id: None,
                selected_for_request: false,
                consent_to_send: false,
                id: ScreenshotId::new("capture-1"),
                source: "capture.png".into(),
                content_sha256: Some(captured_hash.clone()),
                provenance: TaskProvenance {
                    task_id: TaskId::new("task-1"),
                },
                vision: VisionCapability::Available,
                upload: UploadState::Uploaded,
                analysis: ScreenshotAnalysisState::Completed,
            },
        );

        store.prepare_snapshot(&mut snapshot).unwrap();
        assert_eq!(snapshot.media_hashes["capture.png"], captured_hash);
        store.save(&snapshot).unwrap();
        assert!(store
            .load()
            .unwrap()
            .diagnostics
            .contains(&RecoveryDiagnostic::MediaChanged {
                source: "capture.png".into()
            }));
    }

    #[test]
    fn running_paid_calls_recover_as_uncertain_without_replay() {
        let project = TestProject::new("uncertain");
        let store = project.store();
        let mut snapshot = SessionSnapshot::default();
        snapshot.in_flight.insert("task-paid".into());
        store.save(&snapshot).unwrap();
        let loaded = store.load().unwrap();
        let snapshot = loaded.snapshot.unwrap();
        assert!(snapshot.in_flight.is_empty());
        assert!(snapshot.uncertain_calls.contains("task-paid"));
    }

    #[test]
    fn security_related_ids_and_payload_keys_round_trip() {
        let project = TestProject::new("security-identifiers");
        let store = project.store();
        let mut snapshot = SessionSnapshot::default();
        snapshot
            .session
            .new_task("credential-migration", "Reset password", "Sample")
            .unwrap();
        snapshot
            .session
            .active_task_mut()
            .unwrap()
            .propose_action_with_payload(
                "password-reset",
                crate::task_session::ActionKind::Edit,
                "Update credential handling",
                serde_json::json!({"password": "source symbol", "credential-migration": true}),
            )
            .unwrap();
        snapshot
            .validation_receipts
            .insert("password".into(), serde_json::json!({"passed": true}));
        store.save(&snapshot).unwrap();
        assert_eq!(store.load().unwrap().snapshot.unwrap(), snapshot);
    }

    #[test]
    fn private_envelopes_are_rejected_and_erase_retains_media() {
        let project = TestProject::new("privacy");
        let store = project.store();
        let media = project.0.join("owned.png");
        fs::write(&media, b"owned media").unwrap();
        let mut private = SessionSnapshot::default();
        private.execution_receipts.push(ExecutionReceipt {
            task_id: "task".into(),
            action_id: "action".into(),
            receipt: serde_json::json!({"Authorization": "Bearer secret"}),
        });
        assert!(matches!(
            store.save(&private),
            Err(StoreError::PrivacyViolation { .. })
        ));

        store.save(&SessionSnapshot::default()).unwrap();
        let abandoned_atomic = store.state_dir.join(".session.json.A1b2C3");
        let unrelated = store.state_dir.join(".session.json.not-owned");
        fs::write(&abandoned_atomic, b"private draft residue").unwrap();
        fs::write(&unrelated, b"unrelated file").unwrap();
        store.erase().unwrap();
        assert!(!store.state_dir.join(STATE_FILE).exists());
        assert!(!store.state_dir.join(PENDING_FILE).exists());
        assert!(!abandoned_atomic.exists());
        assert!(unrelated.exists());
        assert!(
            media.exists(),
            "privacy erase must not bypass media lifecycle"
        );
    }

    #[test]
    fn privacy_filter_rejects_field_name_variants_and_private_loaded_state() {
        for field in [
            "authorization_headers",
            "provider_raw_envelopes",
            "modelHiddenReasoning",
            "stored_credentials",
        ] {
            let value = Value::Object(
                [(field.to_string(), Value::String("secret".into()))]
                    .into_iter()
                    .collect(),
            );
            assert!(matches!(
                reject_private_fields(&value),
                Err(StoreError::PrivacyViolation { .. })
            ));
        }

        let project = TestProject::new("private-loaded-state");
        let store = project.store();
        fs::create_dir_all(&store.state_dir).unwrap();
        let mut snapshot = serde_json::to_value(SessionSnapshot::default()).unwrap();
        snapshot["validation_receipts"] =
            serde_json::json!({"task": {"provider_raw_envelope": {"secret": true}}});
        let document = serde_json::json!({
            "schema": SCHEMA_NAME,
            "version": CURRENT_VERSION,
            "project": store.project_identity,
            "snapshot": snapshot,
        });
        fs::write(
            store.state_dir.join(STATE_FILE),
            serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load(),
            Err(StoreError::PrivacyViolation { .. })
        ));
    }

    #[test]
    fn privacy_erase_streams_more_than_sixty_four_abandoned_writes() {
        let project = TestProject::new("erase-many-interruptions");
        let store = project.store();
        store.save(&SessionSnapshot::default()).unwrap();
        for index in 0..65 {
            fs::write(
                store.state_dir.join(format!(".session.json.{index:06}")),
                b"private abandoned draft",
            )
            .unwrap();
        }

        store.erase().unwrap();
        assert!(store.load().unwrap().snapshot.is_none());
        assert_eq!(fs::read_dir(&store.state_dir).unwrap().count(), 0);
    }

    #[test]
    fn rejects_unbounded_recovery_metadata_and_malformed_hashes() {
        let project = TestProject::new("metadata-bounds");
        let store = project.store();
        let mut snapshot = SessionSnapshot::default();
        snapshot.uncertain_calls = (0..=MAX_TASKS)
            .map(|index| format!("task-{index}"))
            .collect();
        assert!(matches!(
            store.save(&snapshot),
            Err(StoreError::LimitExceeded {
                field: "uncertain calls",
                ..
            })
        ));

        snapshot.uncertain_calls.clear();
        snapshot
            .media_hashes
            .insert("capture.png".into(), "not-a-sha256".into());
        assert!(matches!(
            store.save(&snapshot),
            Err(StoreError::Corrupt { .. })
        ));
    }
}
