use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stasis_compiler::backend::jit::{JitProcess, JitScalarValue};
use stasis_compiler::backend::state_layout::state_layout_version;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const REPLAY_SCHEMA_VERSION: u32 = 1;
const MAX_REPLAY_FRAMES: usize = 1_000_000;
const MAX_REPLAY_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_HOST_FRAME_VALUES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayReplayConfig {
    Record(PathBuf),
    Replay(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayDocument {
    schema_version: u32,
    identity: ReplayIdentity,
    initial_state: InitialState,
    frames: Vec<ReplayFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayIdentity {
    stasis_version: String,
    release_id: String,
    target: String,
    source_sha256: String,
    state_layout_sha256: String,
    host_i32_count: usize,
    host_f32_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialState {
    values: Vec<StateEntry>,
    state_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateEntry {
    location: StateLocation,
    value: EncodedScalar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StateLocation {
    Scalar {
        path: String,
    },
    Collection {
        path: String,
        field: String,
        index: i32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct EncodedScalar {
    type_name: String,
    bits: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayFrame {
    tick: u64,
    i32_changes: Vec<I32Change>,
    f32_changes: Vec<F32Change>,
    state_sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct I32Change {
    index: usize,
    value: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct F32Change {
    index: usize,
    bits: u32,
}

pub(crate) struct ReplayRecorder {
    output: PathBuf,
    document: ReplayDocument,
    previous_i32: Vec<i32>,
    previous_f32: Vec<f32>,
}

impl ReplayRecorder {
    pub(crate) fn start(
        output: PathBuf,
        jit: &JitProcess,
        host_i32_count: usize,
        host_f32_count: usize,
    ) -> Result<Self, String> {
        if output.exists() {
            return Err(format!(
                "replay recording already exists; refusing to replace {}",
                output.display()
            ));
        }
        let parent = output
            .parent()
            .ok_or_else(|| format!("replay recording has no parent: {}", output.display()))?;
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create replay recording directory {}: {error}",
                parent.display()
            )
        })?;
        Ok(Self {
            output,
            document: ReplayDocument {
                schema_version: REPLAY_SCHEMA_VERSION,
                identity: replay_identity(jit, host_i32_count, host_f32_count)?,
                initial_state: capture_initial_state(jit)?,
                frames: Vec::new(),
            },
            previous_i32: vec![0; host_i32_count],
            previous_f32: vec![0.0; host_f32_count],
        })
    }

    pub(crate) fn begin_tick(
        &mut self,
        tick: u64,
        host_i32: &[i32],
        host_f32: &[f32],
    ) -> Result<(), String> {
        if self.document.frames.len() >= MAX_REPLAY_FRAMES {
            return Err(format!(
                "replay recording exceeds the {MAX_REPLAY_FRAMES}-frame limit"
            ));
        }
        if host_i32.len() != self.previous_i32.len() || host_f32.len() != self.previous_f32.len() {
            return Err("HostFrame size changed while recording replay".to_string());
        }
        self.document.frames.push(ReplayFrame {
            tick,
            i32_changes: diff_i32(&self.previous_i32, host_i32),
            f32_changes: diff_f32(&self.previous_f32, host_f32),
            state_sha256: String::new(),
        });
        self.previous_i32.copy_from_slice(host_i32);
        self.previous_f32.copy_from_slice(host_f32);
        Ok(())
    }

    pub(crate) fn finish_tick(&mut self, jit: &JitProcess) -> Result<(), String> {
        let frame = self
            .document
            .frames
            .last_mut()
            .ok_or_else(|| "replay recorder has no active tick".to_string())?;
        if !frame.state_sha256.is_empty() {
            return Err(format!("replay tick {} was already completed", frame.tick));
        }
        frame.state_sha256 = simulation_state_hash(jit)?;
        Ok(())
    }

    pub(crate) fn discard_tick(&mut self, tick: u64) -> Result<(), String> {
        let frame = self
            .document
            .frames
            .last()
            .ok_or_else(|| format!("replay recorder has no active tick {tick}"))?;
        if frame.tick != tick || !frame.state_sha256.is_empty() {
            return Err(format!(
                "replay tick {tick} is not an unfinished active tick"
            ));
        }
        self.document.frames.pop();
        Ok(())
    }

    pub(crate) fn publish(self) -> Result<PathBuf, String> {
        if self.document.frames.is_empty() {
            return Err("cannot publish a replay without a completed tick".to_string());
        }
        if self
            .document
            .frames
            .last()
            .is_some_and(|frame| frame.state_sha256.is_empty())
        {
            return Err("cannot publish an incomplete replay tick".to_string());
        }
        let bytes = serde_json::to_vec(&self.document)
            .map_err(|error| format!("failed to encode replay recording: {error}"))?;
        if bytes.len() as u64 > MAX_REPLAY_FILE_BYTES {
            return Err(format!(
                "replay recording is too large ({} bytes; maximum {MAX_REPLAY_FILE_BYTES})",
                bytes.len()
            ));
        }
        let temporary = self.output.with_extension(format!(
            "{}.tmp",
            self.output
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("replay")
        ));
        let write_result = (|| -> Result<(), String> {
            let mut file = fs::File::create(&temporary).map_err(|error| {
                format!(
                    "failed to create replay recording {}: {error}",
                    temporary.display()
                )
            })?;
            file.write_all(&bytes).map_err(|error| {
                format!(
                    "failed to write replay recording {}: {error}",
                    temporary.display()
                )
            })?;
            file.sync_all().map_err(|error| {
                format!(
                    "failed to sync replay recording {}: {error}",
                    temporary.display()
                )
            })?;
            stasis_dynload::atomic_rename_no_replace(&temporary, &self.output).map_err(|error| {
                format!(
                    "failed to publish replay recording {}: {error}",
                    self.output.display()
                )
            })
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result?;
        Ok(self.output)
    }
}

pub(crate) struct ReplayPlayer {
    document: ReplayDocument,
    next_frame: usize,
    host_i32: Vec<i32>,
    host_f32: Vec<f32>,
}

impl ReplayPlayer {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path)
            .map_err(|error| format!("failed to inspect replay {}: {error}", path.display()))?;
        if metadata.len() > MAX_REPLAY_FILE_BYTES {
            return Err(format!(
                "replay is too large ({} bytes; maximum {MAX_REPLAY_FILE_BYTES})",
                metadata.len()
            ));
        }
        let source = fs::read(path)
            .map_err(|error| format!("failed to read replay {}: {error}", path.display()))?;
        let document: ReplayDocument = serde_json::from_slice(&source)
            .map_err(|error| format!("failed to parse replay {}: {error}", path.display()))?;
        validate_document(&document)?;
        Ok(Self {
            host_i32: vec![0; document.identity.host_i32_count],
            host_f32: vec![0.0; document.identity.host_f32_count],
            document,
            next_frame: 0,
        })
    }

    pub(crate) fn frame_count(&self) -> u64 {
        self.document.frames.len() as u64
    }

    pub(crate) fn initialize(&self, jit: &JitProcess) -> Result<(), String> {
        let actual = replay_identity(
            jit,
            self.document.identity.host_i32_count,
            self.document.identity.host_f32_count,
        )?;
        if actual.stasis_version != self.document.identity.stasis_version
            || actual.release_id != self.document.identity.release_id
            || actual.target != self.document.identity.target
            || actual.source_sha256 != self.document.identity.source_sha256
            || actual.state_layout_sha256 != self.document.identity.state_layout_sha256
        {
            return Err(format!(
                "replay identity mismatch: recorded {:?}, active {:?}",
                self.document.identity, actual
            ));
        }
        restore_initial_state(jit, &self.document.initial_state)?;
        let actual_hash = simulation_state_hash(jit)?;
        if actual_hash != self.document.initial_state.state_sha256 {
            return Err(format!(
                "replay initial state mismatch: expected {}, found {actual_hash}",
                self.document.initial_state.state_sha256
            ));
        }
        Ok(())
    }

    pub(crate) fn apply_next(
        &mut self,
        tick: u64,
        host_i32: &mut [i32],
        host_f32: &mut [f32],
    ) -> Result<(), String> {
        let frame = self
            .document
            .frames
            .get(self.next_frame)
            .ok_or_else(|| format!("replay has no frame for tick {tick}"))?;
        if frame.tick != tick {
            return Err(format!(
                "replay tick sequence mismatch: expected {}, found {}",
                tick, frame.tick
            ));
        }
        apply_changes(&mut self.host_i32, &mut self.host_f32, frame)?;
        if host_i32.len() != self.host_i32.len() || host_f32.len() != self.host_f32.len() {
            return Err("active HostFrame size does not match replay".to_string());
        }
        host_i32.copy_from_slice(&self.host_i32);
        host_f32.copy_from_slice(&self.host_f32);
        self.next_frame += 1;
        Ok(())
    }

    pub(crate) fn verify_tick(&self, tick: u64, jit: &JitProcess) -> Result<(), String> {
        let frame = self
            .document
            .frames
            .get(self.next_frame.saturating_sub(1))
            .ok_or_else(|| format!("replay has no completed frame for tick {tick}"))?;
        let actual = simulation_state_hash(jit)?;
        if actual != frame.state_sha256 {
            return Err(format!(
                "replay diverged at tick {tick}: expected state {}, found {actual}",
                frame.state_sha256
            ));
        }
        Ok(())
    }
}

fn validate_document(document: &ReplayDocument) -> Result<(), String> {
    if document.schema_version != REPLAY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported replay schema {} (expected {REPLAY_SCHEMA_VERSION})",
            document.schema_version
        ));
    }
    if document.frames.len() > MAX_REPLAY_FRAMES {
        return Err(format!(
            "replay exceeds the {MAX_REPLAY_FRAMES}-frame limit"
        ));
    }
    if document.frames.is_empty() {
        return Err("replay must contain at least one completed tick".to_string());
    }
    if document.identity.host_i32_count == 0
        || document.identity.host_f32_count == 0
        || document.identity.host_i32_count > MAX_HOST_FRAME_VALUES
        || document.identity.host_f32_count > MAX_HOST_FRAME_VALUES
    {
        return Err(format!(
            "replay HostFrame dimensions must be between 1 and {MAX_HOST_FRAME_VALUES}"
        ));
    }
    for (index, frame) in document.frames.iter().enumerate() {
        let tick = index as u64 + 1;
        if frame.tick != tick {
            return Err(format!(
                "replay frames must contain consecutive ticks starting at 1; found {} at position {tick}",
                frame.tick
            ));
        }
        if frame.state_sha256.len() != 64 {
            return Err(format!("replay tick {tick} has an invalid state hash"));
        }
        for change in &frame.i32_changes {
            if change.index >= document.identity.host_i32_count {
                return Err(format!("replay tick {tick} has an out-of-range i32 change"));
            }
        }
        for change in &frame.f32_changes {
            if change.index >= document.identity.host_f32_count {
                return Err(format!("replay tick {tick} has an out-of-range f32 change"));
            }
        }
    }
    Ok(())
}

fn replay_identity(
    jit: &JitProcess,
    host_i32_count: usize,
    host_f32_count: usize,
) -> Result<ReplayIdentity, String> {
    let snapshot = jit
        .program_snapshot()
        .ok_or_else(|| "replay requires a compiled program snapshot".to_string())?;
    let mut files = snapshot.files().to_vec();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut source = Sha256::new();
    source.update(b"stasis.replay.source.v1\0");
    for file in files {
        source.update((file.path.len() as u64).to_le_bytes());
        source.update(file.path.as_bytes());
        source.update((file.content.len() as u64).to_le_bytes());
        source.update(file.content.as_bytes());
    }
    Ok(ReplayIdentity {
        stasis_version: env!("CARGO_PKG_VERSION").to_string(),
        release_id: option_env!("STASIS_RELEASE_ID")
            .unwrap_or("development")
            .to_string(),
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        source_sha256: format!("{:x}", source.finalize()),
        state_layout_sha256: state_layout_version(&jit.state_layout())?,
        host_i32_count,
        host_f32_count,
    })
}

fn capture_initial_state(jit: &JitProcess) -> Result<InitialState, String> {
    validate_supported_state(jit)?;
    let mut values = Vec::new();
    let layout = jit.state_layout();
    let mut scalars = layout.scalars;
    scalars.sort_by(|left, right| left.path.cmp(&right.path));
    for scalar in scalars {
        if is_host_or_presentation_path(&scalar.path) {
            continue;
        }
        let value = jit.read_global_scalar(&scalar.path)?;
        if !is_default(value) {
            values.push(StateEntry {
                location: StateLocation::Scalar { path: scalar.path },
                value: encode_scalar(value),
            });
        }
    }
    let mut collections = layout.collections;
    collections.sort_by(|left, right| left.path.cmp(&right.path));
    for collection in collections {
        if is_host_or_presentation_path(&collection.path) {
            continue;
        }
        let mut fields = collection.fields;
        fields.sort_by(|left, right| left.field.cmp(&right.field));
        for field in fields {
            for index in 0..collection.capacity {
                let value =
                    jit.read_global_collection_scalar(&collection.path, &field.field, index)?;
                if !is_default(value) {
                    values.push(StateEntry {
                        location: StateLocation::Collection {
                            path: collection.path.clone(),
                            field: field.field.clone(),
                            index,
                        },
                        value: encode_scalar(value),
                    });
                }
            }
        }
    }
    Ok(InitialState {
        values,
        state_sha256: simulation_state_hash(jit)?,
    })
}

fn restore_initial_state(jit: &JitProcess, state: &InitialState) -> Result<(), String> {
    validate_supported_state(jit)?;
    let layout = jit.state_layout();
    for scalar in layout.scalars {
        if is_host_or_presentation_path(&scalar.path) {
            continue;
        }
        let current = jit.read_global_scalar(&scalar.path)?;
        jit.write_global_scalar(&scalar.path, default_value(current))?;
    }
    for collection in layout.collections {
        if is_host_or_presentation_path(&collection.path) {
            continue;
        }
        for field in collection.fields {
            for index in 0..collection.capacity {
                let current =
                    jit.read_global_collection_scalar(&collection.path, &field.field, index)?;
                jit.write_global_collection_scalar(
                    &collection.path,
                    &field.field,
                    index,
                    default_value(current),
                )?;
            }
        }
    }
    for entry in &state.values {
        match &entry.location {
            StateLocation::Scalar { path } => {
                let target = jit.read_global_scalar(path)?;
                jit.write_global_scalar(path, decode_scalar(&entry.value, target)?)?;
            }
            StateLocation::Collection { path, field, index } => {
                let target = jit.read_global_collection_scalar(path, field, *index)?;
                jit.write_global_collection_scalar(
                    path,
                    field,
                    *index,
                    decode_scalar(&entry.value, target)?,
                )?;
            }
        }
    }
    Ok(())
}

pub fn simulation_state_hash(jit: &JitProcess) -> Result<String, String> {
    validate_supported_state(jit)?;
    let layout = jit.state_layout();
    let mut hasher = Sha256::new();
    hasher.update(b"stasis.simulation-state.v1\0");
    let mut scalars = layout.scalars;
    scalars.sort_by(|left, right| left.path.cmp(&right.path));
    for scalar in scalars {
        if !is_host_or_presentation_path(&scalar.path) {
            hash_value(
                &mut hasher,
                &scalar.path,
                jit.read_global_scalar(&scalar.path)?,
            );
        }
    }
    let mut collections = layout.collections;
    collections.sort_by(|left, right| left.path.cmp(&right.path));
    for collection in collections {
        if is_host_or_presentation_path(&collection.path) {
            continue;
        }
        let mut fields = collection.fields;
        fields.sort_by(|left, right| left.field.cmp(&right.field));
        for field in fields {
            for index in 0..collection.capacity {
                let label = format!("{}[{index}].{}", collection.path, field.field);
                hash_value(
                    &mut hasher,
                    &label,
                    jit.read_global_collection_scalar(&collection.path, &field.field, index)?,
                );
            }
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_supported_state(jit: &JitProcess) -> Result<(), String> {
    let unsupported = jit
        .state_layout()
        .opaque
        .into_iter()
        .filter(|value| !is_host_or_presentation_path(&value.path))
        .map(|value| value.path)
        .collect::<Vec<_>>();
    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "replay does not support opaque simulation state: {}",
            unsupported.join(", ")
        ))
    }
}

fn is_host_or_presentation_path(path: &str) -> bool {
    path == "host_i32"
        || path == "host_f32"
        || path.starts_with("host_req_")
        || stasis_compiler::backend::state_layout::is_command_buffer_path(path)
}

fn diff_i32(previous: &[i32], current: &[i32]) -> Vec<I32Change> {
    previous
        .iter()
        .zip(current)
        .enumerate()
        .filter_map(|(index, (previous, current))| {
            (previous != current).then_some(I32Change {
                index,
                value: *current,
            })
        })
        .collect()
}

fn diff_f32(previous: &[f32], current: &[f32]) -> Vec<F32Change> {
    previous
        .iter()
        .zip(current)
        .enumerate()
        .filter_map(|(index, (previous, current))| {
            (previous.to_bits() != current.to_bits()).then_some(F32Change {
                index,
                bits: current.to_bits(),
            })
        })
        .collect()
}

fn apply_changes(
    host_i32: &mut [i32],
    host_f32: &mut [f32],
    frame: &ReplayFrame,
) -> Result<(), String> {
    for change in &frame.i32_changes {
        *host_i32
            .get_mut(change.index)
            .ok_or_else(|| format!("replay i32 index {} is out of range", change.index))? =
            change.value;
    }
    for change in &frame.f32_changes {
        *host_f32
            .get_mut(change.index)
            .ok_or_else(|| format!("replay f32 index {} is out of range", change.index))? =
            f32::from_bits(change.bits);
    }
    Ok(())
}

fn is_default(value: JitScalarValue) -> bool {
    match value {
        JitScalarValue::I32(value) => value == 0,
        JitScalarValue::F32(value) => value.to_bits() == 0,
        JitScalarValue::F64(value) => value.to_bits() == 0,
        JitScalarValue::Bool(value) => !value,
        JitScalarValue::U8(value) => value == 0,
        JitScalarValue::U16(value) => value == 0,
        JitScalarValue::U32(value) => value == 0,
    }
}

fn default_value(value: JitScalarValue) -> JitScalarValue {
    match value {
        JitScalarValue::I32(_) => JitScalarValue::I32(0),
        JitScalarValue::F32(_) => JitScalarValue::F32(0.0),
        JitScalarValue::F64(_) => JitScalarValue::F64(0.0),
        JitScalarValue::Bool(_) => JitScalarValue::Bool(false),
        JitScalarValue::U8(_) => JitScalarValue::U8(0),
        JitScalarValue::U16(_) => JitScalarValue::U16(0),
        JitScalarValue::U32(_) => JitScalarValue::U32(0),
    }
}

fn encode_scalar(value: JitScalarValue) -> EncodedScalar {
    let (type_name, bits) = match value {
        JitScalarValue::I32(value) => ("i32", format!("{:08x}", value as u32)),
        JitScalarValue::F32(value) => ("f32", format!("{:08x}", value.to_bits())),
        JitScalarValue::F64(value) => ("f64", format!("{:016x}", value.to_bits())),
        JitScalarValue::Bool(value) => ("bool", format!("{:02x}", u8::from(value))),
        JitScalarValue::U8(value) => ("u8", format!("{value:02x}")),
        JitScalarValue::U16(value) => ("u16", format!("{value:04x}")),
        JitScalarValue::U32(value) => ("u32", format!("{value:08x}")),
    };
    EncodedScalar {
        type_name: type_name.to_string(),
        bits,
    }
}

fn decode_scalar(value: &EncodedScalar, target: JitScalarValue) -> Result<JitScalarValue, String> {
    if value.type_name != target.type_name() {
        return Err(format!(
            "replay state type mismatch: recorded {}, active {}",
            value.type_name,
            target.type_name()
        ));
    }
    let bits = u64::from_str_radix(&value.bits, 16)
        .map_err(|error| format!("invalid replay scalar bits '{}': {error}", value.bits))?;
    match target {
        JitScalarValue::I32(_) => u32::try_from(bits)
            .map(|value| JitScalarValue::I32(value as i32))
            .map_err(|_| "replay i32 bits are out of range".to_string()),
        JitScalarValue::F32(_) => u32::try_from(bits)
            .map(|value| JitScalarValue::F32(f32::from_bits(value)))
            .map_err(|_| "replay f32 bits are out of range".to_string()),
        JitScalarValue::F64(_) => Ok(JitScalarValue::F64(f64::from_bits(bits))),
        JitScalarValue::Bool(_) if bits <= 1 => Ok(JitScalarValue::Bool(bits == 1)),
        JitScalarValue::Bool(_) => Err("replay bool bits are out of range".to_string()),
        JitScalarValue::U8(_) => u8::try_from(bits)
            .map(JitScalarValue::U8)
            .map_err(|_| "replay u8 bits are out of range".to_string()),
        JitScalarValue::U16(_) => u16::try_from(bits)
            .map(JitScalarValue::U16)
            .map_err(|_| "replay u16 bits are out of range".to_string()),
        JitScalarValue::U32(_) => u32::try_from(bits)
            .map(JitScalarValue::U32)
            .map_err(|_| "replay u32 bits are out of range".to_string()),
    }
}

fn hash_value(hasher: &mut Sha256, path: &str, value: JitScalarValue) {
    hasher.update((path.len() as u64).to_le_bytes());
    hasher.update(path.as_bytes());
    match value {
        JitScalarValue::I32(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        JitScalarValue::F32(value) => {
            hasher.update([2]);
            hasher.update(value.to_bits().to_le_bytes());
        }
        JitScalarValue::F64(value) => {
            hasher.update([3]);
            hasher.update(value.to_bits().to_le_bytes());
        }
        JitScalarValue::Bool(value) => hasher.update([4, u8::from(value)]),
        JitScalarValue::U8(value) => hasher.update([5, value]),
        JitScalarValue::U16(value) => {
            hasher.update([6]);
            hasher.update(value.to_le_bytes());
        }
        JitScalarValue::U32(value) => {
            hasher.update([7]);
            hasher.update(value.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn host_diffs_reconstruct_exact_bits_and_zero_transitions() {
        let initial_i32 = vec![0, 7, 0];
        let initial_f32 = vec![0.0, -0.0, f32::from_bits(0x7fc0_0042)];
        let next_i32 = vec![0, 0, 9];
        let next_f32 = vec![1.5, 0.0, f32::from_bits(0x7fc0_0042)];
        let first = ReplayFrame {
            tick: 1,
            i32_changes: diff_i32(&[0; 3], &initial_i32),
            f32_changes: diff_f32(&[0.0; 3], &initial_f32),
            state_sha256: "0".repeat(64),
        };
        let second = ReplayFrame {
            tick: 2,
            i32_changes: diff_i32(&initial_i32, &next_i32),
            f32_changes: diff_f32(&initial_f32, &next_f32),
            state_sha256: "0".repeat(64),
        };
        assert_eq!(second.i32_changes[0], I32Change { index: 1, value: 0 });
        let mut rebuilt_i32 = vec![0; 3];
        let mut rebuilt_f32 = vec![0.0; 3];
        apply_changes(&mut rebuilt_i32, &mut rebuilt_f32, &first).expect("first diff");
        apply_changes(&mut rebuilt_i32, &mut rebuilt_f32, &second).expect("second diff");
        assert_eq!(rebuilt_i32, next_i32);
        assert_eq!(
            rebuilt_f32
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            next_f32
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn scalar_encoding_preserves_exact_numeric_bits() {
        for value in [
            JitScalarValue::I32(-7),
            JitScalarValue::F32(f32::from_bits(0x8000_0000)),
            JitScalarValue::F64(f64::from_bits(0x7ff8_0000_0000_0042)),
            JitScalarValue::Bool(true),
            JitScalarValue::U8(255),
            JitScalarValue::U16(65_535),
            JitScalarValue::U32(u32::MAX),
        ] {
            let encoded = encode_scalar(value);
            let decoded = decode_scalar(&encoded, value).expect("decode exact scalar bits");
            assert_eq!(encode_scalar(decoded), encoded);
        }
    }

    #[test]
    fn unfinished_tick_can_be_discarded_after_guest_shutdown() {
        let mut recorder = ReplayRecorder {
            output: PathBuf::from("unused.replay.json"),
            document: ReplayDocument {
                schema_version: REPLAY_SCHEMA_VERSION,
                identity: ReplayIdentity {
                    stasis_version: "test".to_string(),
                    release_id: "test".to_string(),
                    target: "test".to_string(),
                    source_sha256: "0".repeat(64),
                    state_layout_sha256: "0".repeat(64),
                    host_i32_count: 2,
                    host_f32_count: 1,
                },
                initial_state: InitialState {
                    values: Vec::new(),
                    state_sha256: "0".repeat(64),
                },
                frames: Vec::new(),
            },
            previous_i32: vec![0; 2],
            previous_f32: vec![0.0; 1],
        };
        recorder
            .begin_tick(1, &[7, 0], &[1.0])
            .expect("begin unfinished tick");
        recorder.discard_tick(1).expect("discard unfinished tick");
        assert!(recorder.document.frames.is_empty());
    }

    #[test]
    fn presentation_and_host_paths_are_excluded_from_simulation_state() {
        for path in [
            "host_i32",
            "host_f32",
            "host_req_flags",
            "gfx_cmd_i32",
            "render_cmd_i32",
            "audio_cmd_i32",
            "cmd_i32",
            "world.render_cmd_i32",
            "world.cmd_i32",
        ] {
            assert!(is_host_or_presentation_path(path), "{path}");
        }
        assert!(!is_host_or_presentation_path("world.score"));
    }

    #[test]
    fn recorded_session_rebuilds_state_and_detects_divergence() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "stasis-record-replay-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("run.replay.json");
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "main.stasis",
            "global score: i32; function main(): i32 { score = 4; return 0; } function tick(): i32 { score += 3; return 0; } function render(): i32 { return 0; }",
        );
        jit.compile().expect("compile replay fixture");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

        let mut recorder = ReplayRecorder::start(path.clone(), &jit, 3, 2).expect("start recorder");
        assert_eq!(recorder.document.initial_state.values.len(), 1);
        recorder
            .begin_tick(1, &[7, 0, 1], &[1.5, 0.0])
            .expect("record input diff");
        assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
        assert_eq!(jit.execute_i32_noarg_by_name("render"), Ok(0));
        recorder.finish_tick(&jit).expect("record state hash");
        recorder.publish().expect("publish replay");

        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));
        let mut player = ReplayPlayer::load(&path).expect("load replay");
        player.initialize(&jit).expect("restore initial state");
        let mut host_i32 = vec![0; 3];
        let mut host_f32 = vec![0.0; 2];
        player
            .apply_next(1, &mut host_i32, &mut host_f32)
            .expect("rebuild HostFrame");
        assert_eq!(host_i32, [7, 0, 1]);
        assert_eq!(host_f32, [1.5, 0.0]);
        assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
        assert_eq!(jit.execute_i32_noarg_by_name("render"), Ok(0));
        player.verify_tick(1, &jit).expect("matching replay state");

        jit.write_global_scalar("score", JitScalarValue::I32(99))
            .expect("force divergence");
        assert!(player
            .verify_tick(1, &jit)
            .expect_err("divergence")
            .contains("diverged at tick 1"));
        fs::remove_dir_all(directory).ok();
    }
}
