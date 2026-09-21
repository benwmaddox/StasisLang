use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stasis_compiler::backend::jit::{JitProcess, JitScalarValue};
use stasis_compiler::backend::program_snapshot::{ProgramExternImport, ProgramSnapshot};
use stasis_compiler::backend::state_layout::state_layout_version;
use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const REPLAY_SCHEMA_VERSION: u32 = 1;
pub(crate) const COMPACT_REPLAY_SCHEMA_VERSION: u32 = 2;
const MAX_REPLAY_FRAMES: usize = 1_000_000;
const MAX_REPLAY_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_HOST_FRAME_VALUES: usize = 4_096;
const MAX_COMPACT_CHECKPOINTS: usize = 4_096;
const MAX_COMPACT_FIELD_TEXT_BYTES: usize = 4_096;
const MAX_COMPACT_CHANGES_PER_SEGMENT: usize = MAX_HOST_FRAME_VALUES * 2;
const COMPACT_CHECKPOINT_INTERVAL: u64 = 256;
const MAX_COMPACT_DIAGNOSTIC_BYTES: usize = 512;
const COMPACT_HASH_SCOPE: &str = "simulation_after_tick";
const HOST_FRAME_SCHEMA_VERSION: u32 = 4;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CompactHashScope {
    SimulationAfterTick,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactI32Field {
    pub(crate) slot: usize,
    pub(crate) index: usize,
    pub(crate) path: String,
    pub(crate) family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactF32Field {
    pub(crate) slot: usize,
    pub(crate) index: usize,
    pub(crate) path: String,
    pub(crate) family: String,
}

impl CompactI32Field {
    pub(crate) fn new(
        slot: usize,
        index: usize,
        path: impl Into<String>,
        family: impl Into<String>,
    ) -> Self {
        Self {
            slot,
            index,
            path: path.into(),
            family: family.into(),
        }
    }
}

impl CompactF32Field {
    pub(crate) fn new(
        slot: usize,
        index: usize,
        path: impl Into<String>,
        family: impl Into<String>,
    ) -> Self {
        Self {
            slot,
            index,
            path: path.into(),
            family: family.into(),
        }
    }
}

pub(crate) fn compact_observed_fields(
    jit: &JitProcess,
) -> Result<(Vec<CompactI32Field>, Vec<CompactF32Field>), String> {
    let usage = jit
        .program_snapshot()
        .ok_or_else(|| "record/replay compile produced no ProgramSnapshot".to_string())?
        .host_frame_input_usage();
    let observed_i32 = usage
        .i32_fields()
        .iter()
        .map(|field| {
            CompactI32Field::new(
                field.slot,
                field.index,
                field.path.clone(),
                field.family.as_str(),
            )
        })
        .collect();
    let observed_f32 = usage
        .f32_fields()
        .iter()
        .map(|field| {
            CompactF32Field::new(
                field.slot,
                field.index,
                field.path.clone(),
                field.family.as_str(),
            )
        })
        .collect();
    Ok((observed_i32, observed_f32))
}

pub(crate) fn validate_compact_replay_contract(jit: &JitProcess) -> Result<(), String> {
    let snapshot = jit
        .program_snapshot()
        .ok_or_else(|| "record/replay compile produced no ProgramSnapshot".to_string())?;
    let mut unsupported = BTreeSet::new();
    for summary in snapshot.data_flow_summaries() {
        let function = snapshot.functions().iter().find(|function| {
            function.name == summary.function
                && function.source_range.start == summary.source_start
                && function.source_range.end == summary.source_end
                && snapshot
                    .files()
                    .get(function.file_id as usize)
                    .is_some_and(|file| file.path == summary.file)
        });
        let Some(function) = function else {
            return Err(format!(
                "compact replay could not map effect summary for '{}'; refusing an incomplete determinism audit",
                summary.function
            ));
        };
        if !snapshot.reachable_function_ids().contains(&function.id) {
            continue;
        }
        for effect in &summary.direct.host_effects {
            if compact_effect_is_unsupported(snapshot, &effect.function, &effect.capability) {
                unsupported.insert(format!(
                    "{} -> {} ({})",
                    summary.function, effect.function, effect.capability
                ));
            }
        }
    }
    if unsupported.is_empty() {
        return Ok(());
    }
    let mut diagnostic = format!(
        "compact replay does not virtualize reachable host observations: {}",
        unsupported.into_iter().collect::<Vec<_>>().join(", ")
    );
    diagnostic.truncate(MAX_COMPACT_DIAGNOSTIC_BYTES);
    Err(diagnostic)
}

fn compact_effect_is_unsupported(
    snapshot: &ProgramSnapshot,
    function: &str,
    capability: &str,
) -> bool {
    match capability {
        "memory" | "graphics" | "audio" => {
            !compact_effect_has_safe_import(snapshot, function, capability)
        }
        "platform" => !matches!(
            function,
            "print_i32" | "print_int" | "print_char" | "print_string"
        ),
        "storage" | "network" | "nondeterministic" | "unknown" | "code_swap" => true,
        _ => true,
    }
}

fn compact_effect_has_safe_import(
    snapshot: &ProgramSnapshot,
    function: &str,
    capability: &str,
) -> bool {
    let unqualified = function.rsplit('.').next().unwrap_or(function);
    let imports = snapshot
        .extern_imports()
        .iter()
        .filter(|import| import.name == function || import.name == unqualified)
        .collect::<Vec<_>>();
    !imports.is_empty()
        && imports
            .into_iter()
            .all(|import| compact_import_is_safe(import, capability))
}

fn compact_import_is_safe(import: &ProgramExternImport, capability: &str) -> bool {
    match capability {
        "memory" => {
            import.returns_void
                && import.params.len() == 5
                && matches!(
                    (import.name.as_str(), import.symbol.as_str()),
                    (
                        "sys_memcpy_u8",
                        "sys_memcpy_u8" | "stasis_jit_sys_memcpy_u8"
                    ) | (
                        "sys_memcpy_i32",
                        "sys_memcpy_i32" | "stasis_jit_sys_memcpy_i32"
                    ) | (
                        "sys_memcpy_f32",
                        "sys_memcpy_f32" | "stasis_jit_sys_memcpy_f32"
                    ) | (
                        "sys_memmove_u8",
                        "sys_memmove_u8" | "stasis_jit_sys_memmove_u8"
                    ) | (
                        "sys_memmove_i32",
                        "sys_memmove_i32" | "stasis_jit_sys_memmove_i32"
                    ) | (
                        "sys_memmove_f32",
                        "sys_memmove_f32" | "stasis_jit_sys_memmove_f32"
                    )
                )
        }
        // Synchronous asset construction/measurement is reproducible under the
        // exact runtime and asset identities. Release imports must be void.
        "graphics" => match import.symbol.as_str() {
            "load_font"
            | "stasis_load_font"
            | "stasis_jit_load_font"
            | "measure_text"
            | "stasis_measure_text"
            | "stasis_jit_measure_text"
            | "stasis_jit_sprite_load_from"
            | "stasis_jit_text_run_load_from"
            | "stasis_jit_text_run_replace_from" => true,
            "gfx_release_font"
            | "stasis_gfx_release_font"
            | "stasis_jit_gfx_release_font"
            | "gfx_release_sprite"
            | "stasis_gfx_release_sprite"
            | "stasis_jit_gfx_release_sprite" => import.returns_void,
            _ => false,
        },
        // Only exact void output-only audio imports are safe. Initialization,
        // requests, playback/queue handles, and status remain rejected.
        "audio" => {
            import.returns_void
                && matches!(
                    import.symbol.as_str(),
                    "audio_shutdown"
                        | "stasis_audio_shutdown"
                        | "stasis_jit_audio_shutdown"
                        | "audio_stop"
                        | "stasis_audio_stop"
                        | "stasis_jit_audio_stop"
                        | "audio_voice_set_paused"
                        | "stasis_audio_voice_set_paused"
                        | "stasis_jit_audio_voice_set_paused"
                        | "audio_voice_set_volume_pan"
                        | "stasis_audio_voice_set_volume_pan"
                        | "stasis_jit_audio_voice_set_volume_pan"
                        | "audio_release"
                        | "stasis_audio_release"
                )
        }
        _ => false,
    }
}

/// Metadata supplied by the host/compiler integration for a compact replay.
///
/// The recorder deliberately does not derive the observed-input set. The
/// caller owns whole-game reachability analysis and passes the resulting
/// descriptors to `ReplayRecorder::start_compact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactReplayMetadata {
    pub(crate) runtime_sha256: String,
    pub(crate) asset_manifest_sha256: Option<String>,
    pub(crate) host_schema_version: u32,
    pub(crate) tick_rate_hz: u32,
    pub(crate) determinism_profile: String,
    pub(crate) controller_schema_version: Option<u32>,
}

impl Default for CompactReplayMetadata {
    fn default() -> Self {
        Self {
            runtime_sha256: compact_runtime_identity(),
            asset_manifest_sha256: None,
            host_schema_version: HOST_FRAME_SCHEMA_VERSION,
            tick_rate_hz: 60,
            determinism_profile: "input_only_no_external_observations".to_string(),
            controller_schema_version: None,
        }
    }
}

fn compact_runtime_identity() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"stasis.replay.runtime.v1\0");
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(
        option_env!("STASIS_RELEASE_ID")
            .unwrap_or("development")
            .as_bytes(),
    );
    format!("{:x}", hasher.finalize())
}

fn compact_compiler_layout_identity(jit: &JitProcess) -> Result<String, String> {
    let snapshot = jit
        .program_snapshot()
        .ok_or_else(|| "record/replay compile produced no ProgramSnapshot".to_string())?;
    Ok(snapshot
        .compiler_layout_digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactReplayIdentity {
    stasis_version: String,
    release_id: String,
    target: String,
    source_sha256: String,
    state_layout_sha256: String,
    compiler_layout_sha256: String,
    runtime_sha256: String,
    #[serde(default)]
    asset_manifest_sha256: Option<String>,
    host_schema_version: u32,
    host_i32_count: usize,
    host_f32_count: usize,
    input_usage_sha256: String,
    tick_rate_hz: u32,
    hash_scope: CompactHashScope,
    determinism_profile: String,
    #[serde(default)]
    controller_schema_version: Option<u32>,
    observed_i32: Vec<CompactI32Field>,
    observed_f32: Vec<CompactF32Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactInputBaseline {
    i32_values: Vec<i32>,
    f32_bits: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactI32Change {
    slot: usize,
    value: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactF32Change {
    slot: usize,
    bits: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactInputSegment {
    tick_gap: u64,
    run_ticks: u64,
    i32_changes: Vec<CompactI32Change>,
    f32_changes: Vec<CompactF32Change>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactReplayCheckpoint {
    tick: u64,
    state_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CompactReplayFinal {
    tick: u64,
    state_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactReplayDocument {
    schema_version: u32,
    identity: CompactReplayIdentity,
    initial_state: InitialState,
    initial_input: CompactInputBaseline,
    segments: Vec<CompactInputSegment>,
    checkpoints: Vec<CompactReplayCheckpoint>,
    total_ticks: u64,
    final_state: CompactReplayFinal,
}

#[derive(Debug, Clone)]
enum VersionedReplayDocument {
    LegacyV1(ReplayDocument),
    CompactV2(CompactReplayDocument),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactInputSnapshot {
    i32_values: Vec<i32>,
    f32_bits: Vec<u32>,
}

impl CompactReplayDocument {
    fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        validate_compact_document(self)?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("failed to encode compact replay recording: {error}"))?;
        if bytes.len() as u64 > MAX_REPLAY_FILE_BYTES {
            return Err(format!(
                "compact replay is too large ({} bytes; maximum {MAX_REPLAY_FILE_BYTES})",
                bytes.len()
            ));
        }
        Ok(bytes)
    }
}

fn decode_versioned_replay(source: &[u8]) -> Result<VersionedReplayDocument, String> {
    if source.len() as u64 > MAX_REPLAY_FILE_BYTES {
        return Err(format!(
            "replay is too large ({} bytes; maximum {MAX_REPLAY_FILE_BYTES})",
            source.len()
        ));
    }
    let schema = probe_replay_schema(source)?;
    match schema {
        value if value == u64::from(REPLAY_SCHEMA_VERSION) => {
            let document: ReplayDocument = serde_json::from_slice(source)
                .map_err(|error| format!("failed to parse schema-1 replay: {error}"))?;
            validate_document(&document)?;
            Ok(VersionedReplayDocument::LegacyV1(document))
        }
        value if value == u64::from(COMPACT_REPLAY_SCHEMA_VERSION) => {
            let document: CompactReplayDocument = serde_json::from_slice(source)
                .map_err(|error| format!("failed to parse compact schema-2 replay: {error}"))?;
            validate_compact_document(&document)?;
            Ok(VersionedReplayDocument::CompactV2(document))
        }
        value => Err(format!("unsupported replay schema {value}")),
    }
}

fn probe_replay_schema(source: &[u8]) -> Result<u64, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(source);
    let schema = deserializer
        .deserialize_any(ReplaySchemaProbe)
        .map_err(|error| format!("failed to parse replay document: {error}"))?
        .ok_or_else(|| "replay document is missing an integer schema_version".to_string())?;
    deserializer
        .end()
        .map_err(|error| format!("failed to parse replay document: {error}"))?;
    Ok(schema)
}

struct ReplaySchemaProbe;

impl<'de> Visitor<'de> for ReplaySchemaProbe {
    type Value = Option<u64>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a replay JSON object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        let mut schema = None;
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object key {key:?}"
                )));
            }
            if key == "schema_version" {
                schema = Some(map.next_value::<u64>()?);
            } else {
                map.next_value_seed(DuplicateKeySeed)?;
            }
        }
        Ok(schema)
    }
}

struct DuplicateKeySeed;

impl<'de> DeserializeSeed<'de> for DuplicateKeySeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DuplicateKeyVisitor)
    }
}

struct DuplicateKeyVisitor;

impl<'de> Visitor<'de> for DuplicateKeyVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_bytes<E>(self, _value: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_byte_buf<E>(self, _value: Vec<u8>) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element_seed(DuplicateKeySeed)?.is_some() {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object key {key:?}"
                )));
            }
            map.next_value_seed(DuplicateKeySeed)?;
        }
        Ok(())
    }
}

fn validate_compact_document(document: &CompactReplayDocument) -> Result<(), String> {
    if document.schema_version != COMPACT_REPLAY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported compact replay schema {} (expected {COMPACT_REPLAY_SCHEMA_VERSION})",
            document.schema_version
        ));
    }
    let identity = &document.identity;
    for (label, value) in [
        ("stasis_version", identity.stasis_version.as_str()),
        ("release_id", identity.release_id.as_str()),
        ("target", identity.target.as_str()),
        ("determinism_profile", identity.determinism_profile.as_str()),
    ] {
        validate_compact_field_text(&format!("identity {label}"), value)?;
    }
    for (label, value) in [
        ("source_sha256", identity.source_sha256.as_str()),
        ("state_layout_sha256", identity.state_layout_sha256.as_str()),
        (
            "compiler_layout_sha256",
            identity.compiler_layout_sha256.as_str(),
        ),
        ("runtime_sha256", identity.runtime_sha256.as_str()),
        ("input_usage_sha256", identity.input_usage_sha256.as_str()),
    ] {
        validate_compact_hash(label, value)?;
    }
    if let Some(hash) = identity.asset_manifest_sha256.as_deref() {
        validate_compact_hash("asset_manifest_sha256", hash)?;
    }
    if identity.host_schema_version == 0 {
        return Err("compact replay host_schema_version must be positive".to_string());
    }
    if identity.host_i32_count == 0
        || identity.host_f32_count == 0
        || identity.host_i32_count > MAX_HOST_FRAME_VALUES
        || identity.host_f32_count > MAX_HOST_FRAME_VALUES
    {
        return Err(format!(
            "compact replay HostFrame dimensions must be between 1 and {MAX_HOST_FRAME_VALUES}"
        ));
    }
    if identity.tick_rate_hz == 0 {
        return Err("compact replay tick_rate_hz must be positive".to_string());
    }
    if identity.hash_scope != CompactHashScope::SimulationAfterTick {
        return Err(format!(
            "compact replay hash_scope must be {COMPACT_HASH_SCOPE}"
        ));
    }
    if let Some(version) = identity.controller_schema_version {
        if version == 0 {
            return Err(
                "compact replay controller_schema_version must be positive when present"
                    .to_string(),
            );
        }
    }
    validate_compact_i32_fields(&identity.observed_i32, identity.host_i32_count)?;
    validate_compact_f32_fields(&identity.observed_f32, identity.host_f32_count)?;
    let actual_usage_hash =
        compact_input_usage_hash(&identity.observed_i32, &identity.observed_f32);
    if identity.input_usage_sha256 != actual_usage_hash {
        return Err(format!(
            "compact replay input_usage_sha256 does not match its observed field descriptors: expected {}, found {}",
            identity.input_usage_sha256, actual_usage_hash
        ));
    }
    validate_compact_initial_state(&document.initial_state)?;
    if document.initial_input.i32_values.len() != identity.observed_i32.len()
        || document.initial_input.f32_bits.len() != identity.observed_f32.len()
    {
        return Err(
            "compact replay initial_input lengths must match observed field descriptors"
                .to_string(),
        );
    }
    if document.total_ticks == 0 || document.total_ticks > MAX_REPLAY_FRAMES as u64 {
        return Err(format!(
            "compact replay total_ticks must be between 1 and {MAX_REPLAY_FRAMES}"
        ));
    }
    if document.segments.is_empty() {
        return Err("compact replay must contain at least one input segment".to_string());
    }
    if document.segments.len() > MAX_REPLAY_FRAMES {
        return Err(format!(
            "compact replay exceeds the {MAX_REPLAY_FRAMES}-segment limit"
        ));
    }
    let mut snapshot = CompactInputSnapshot {
        i32_values: document.initial_input.i32_values.clone(),
        f32_bits: document.initial_input.f32_bits.clone(),
    };
    let mut cursor = 0u64;
    for (segment_index, segment) in document.segments.iter().enumerate() {
        if segment_index == 0 && segment.tick_gap != 0 {
            return Err("compact replay first segment must have tick_gap=0".to_string());
        }
        if segment.run_ticks == 0 {
            return Err(format!(
                "compact replay segment {segment_index} must have run_ticks >= 1"
            ));
        }
        let change_count = segment
            .i32_changes
            .len()
            .checked_add(segment.f32_changes.len())
            .ok_or_else(|| {
                format!("compact replay segment {segment_index} change count overflows")
            })?;
        if change_count > MAX_COMPACT_CHANGES_PER_SEGMENT {
            return Err(format!(
                "compact replay segment {segment_index} has too many changes"
            ));
        }
        let gap_end = cursor
            .checked_add(segment.tick_gap)
            .ok_or_else(|| format!("compact replay segment {segment_index} tick_gap overflows"))?;
        let run_end = gap_end
            .checked_add(segment.run_ticks)
            .ok_or_else(|| format!("compact replay segment {segment_index} run_ticks overflows"))?;
        if run_end > document.total_ticks {
            return Err(format!(
                "compact replay segment {segment_index} exceeds total_ticks"
            ));
        }
        validate_and_apply_compact_changes(&mut snapshot, segment, segment_index)?;
        cursor = run_end;
    }
    if cursor != document.total_ticks {
        return Err(format!(
            "compact replay segment coverage ends at tick {cursor}, expected {}",
            document.total_ticks
        ));
    }
    if document.checkpoints.len() > MAX_COMPACT_CHECKPOINTS {
        return Err(format!(
            "compact replay exceeds the {MAX_COMPACT_CHECKPOINTS}-checkpoint limit"
        ));
    }
    let expected_checkpoint_count = document.total_ticks / COMPACT_CHECKPOINT_INTERVAL;
    if document.checkpoints.len() as u64 != expected_checkpoint_count {
        return Err(format!(
            "compact replay must contain exactly {expected_checkpoint_count} checkpoints for {} ticks",
            document.total_ticks
        ));
    }
    for (index, checkpoint) in document.checkpoints.iter().enumerate() {
        let expected_tick = (index as u64 + 1) * COMPACT_CHECKPOINT_INTERVAL;
        if checkpoint.tick != expected_tick {
            return Err(format!(
                "compact replay checkpoint {} must be at tick {expected_tick}, found {}",
                index, checkpoint.tick
            ));
        }
        validate_compact_hash("checkpoint state_sha256", &checkpoint.state_sha256)?;
    }
    if document.final_state.tick != document.total_ticks {
        return Err(format!(
            "compact replay final tick {} does not equal total_ticks {}",
            document.final_state.tick, document.total_ticks
        ));
    }
    validate_compact_hash("final state_sha256", &document.final_state.state_sha256)?;
    if document.checkpoints.last().is_some_and(|checkpoint| {
        checkpoint.tick == document.total_ticks
            && checkpoint.state_sha256 != document.final_state.state_sha256
    }) {
        return Err("compact replay final checkpoint does not match final state".to_string());
    }
    Ok(())
}

fn validate_compact_initial_state(state: &InitialState) -> Result<(), String> {
    validate_compact_hash("initial state_sha256", &state.state_sha256)?;
    if state.values.len() > MAX_REPLAY_FRAMES {
        return Err(format!(
            "compact replay initial state exceeds the {MAX_REPLAY_FRAMES}-entry limit"
        ));
    }
    for (entry_index, entry) in state.values.iter().enumerate() {
        match &entry.location {
            StateLocation::Scalar { path } => {
                validate_compact_field_text("initial state scalar path", path)?;
            }
            StateLocation::Collection { path, field, index } => {
                validate_compact_field_text("initial state collection path", path)?;
                validate_compact_field_text("initial state collection field", field)?;
                if *index < 0 {
                    return Err(format!(
                        "compact replay initial state entry {entry_index} has a negative collection index"
                    ));
                }
            }
        }
        validate_compact_field_text("initial state type", &entry.value.type_name)?;
        validate_compact_scalar_encoding(entry_index, &entry.value)?;
    }
    Ok(())
}

fn validate_compact_scalar_encoding(
    entry_index: usize,
    value: &EncodedScalar,
) -> Result<(), String> {
    let width = match value.type_name.as_str() {
        "i32" | "f32" | "u32" => 8,
        "f64" => 16,
        "bool" | "u8" => 2,
        "u16" => 4,
        other => {
            return Err(format!(
                "compact replay initial state entry {entry_index} has unsupported scalar type {other:?}"
            ));
        }
    };
    let label = format!("initial state entry {entry_index} bits");
    validate_compact_bit_text(&label, &value.bits)?;
    if value.bits.len() != width {
        return Err(format!(
            "compact replay {label} must contain exactly {width} lowercase hexadecimal characters"
        ));
    }
    let bits = u64::from_str_radix(&value.bits, 16).map_err(|error| {
        format!("compact replay {label} is not a valid hexadecimal scalar: {error}")
    })?;
    match value.type_name.as_str() {
        "bool" if bits > 1 => {
            return Err(format!(
                "compact replay {label} bool value must be 00 or 01"
            ));
        }
        "u8" if bits > u64::from(u8::MAX) => {
            return Err(format!("compact replay {label} u8 value is out of range"));
        }
        "u16" if bits > u64::from(u16::MAX) => {
            return Err(format!("compact replay {label} u16 value is out of range"));
        }
        _ => {}
    }
    Ok(())
}

fn validate_compact_bit_text(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("compact replay {label} must not be empty"));
    }
    if value.len() > MAX_COMPACT_FIELD_TEXT_BYTES {
        return Err(format!(
            "compact replay {label} exceeds {MAX_COMPACT_FIELD_TEXT_BYTES} bytes"
        ));
    }
    if !value
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(format!(
            "compact replay {label} must contain lowercase hexadecimal text"
        ));
    }
    Ok(())
}

fn validate_compact_i32_fields(
    fields: &[CompactI32Field],
    host_count: usize,
) -> Result<(), String> {
    let mut previous_index = None;
    for (slot, field) in fields.iter().enumerate() {
        if field.slot != slot {
            return Err(format!(
                "compact replay i32 descriptor slot {} is not canonical position {slot}",
                field.slot
            ));
        }
        if field.index >= host_count {
            return Err(format!(
                "compact replay i32 descriptor slot {slot} index {} is out of range",
                field.index
            ));
        }
        if previous_index.is_some_and(|previous| field.index <= previous) {
            return Err("compact replay i32 descriptors must be sorted and unique".to_string());
        }
        validate_compact_field_text("i32 descriptor path", &field.path)?;
        validate_compact_field_text("i32 descriptor family", &field.family)?;
        previous_index = Some(field.index);
    }
    Ok(())
}

fn validate_compact_f32_fields(
    fields: &[CompactF32Field],
    host_count: usize,
) -> Result<(), String> {
    let mut previous_index = None;
    for (slot, field) in fields.iter().enumerate() {
        if field.slot != slot {
            return Err(format!(
                "compact replay f32 descriptor slot {} is not canonical position {slot}",
                field.slot
            ));
        }
        if field.index >= host_count {
            return Err(format!(
                "compact replay f32 descriptor slot {slot} index {} is out of range",
                field.index
            ));
        }
        if previous_index.is_some_and(|previous| field.index <= previous) {
            return Err("compact replay f32 descriptors must be sorted and unique".to_string());
        }
        validate_compact_field_text("f32 descriptor path", &field.path)?;
        validate_compact_field_text("f32 descriptor family", &field.family)?;
        previous_index = Some(field.index);
    }
    Ok(())
}

fn validate_compact_field_text(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("compact replay {label} must not be empty"));
    }
    if value.len() > MAX_COMPACT_FIELD_TEXT_BYTES {
        return Err(format!(
            "compact replay {label} exceeds {MAX_COMPACT_FIELD_TEXT_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_compact_hash(label: &str, value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(format!("compact replay {label} must be lowercase 64-hex"));
    }
    Ok(())
}

fn validate_and_apply_compact_changes(
    snapshot: &mut CompactInputSnapshot,
    segment: &CompactInputSegment,
    segment_index: usize,
) -> Result<(), String> {
    let mut previous_slot = None;
    for change in &segment.i32_changes {
        if change.slot >= snapshot.i32_values.len() {
            return Err(format!(
                "compact replay segment {segment_index} has an out-of-range i32 slot {}",
                change.slot
            ));
        }
        if previous_slot.is_some_and(|previous| change.slot <= previous) {
            return Err(format!(
                "compact replay segment {segment_index} i32 changes must be sorted and unique"
            ));
        }
        if snapshot.i32_values[change.slot] == change.value {
            return Err(format!(
                "compact replay segment {segment_index} i32 change at slot {} is redundant",
                change.slot
            ));
        }
        previous_slot = Some(change.slot);
    }
    let mut previous_slot = None;
    for change in &segment.f32_changes {
        if change.slot >= snapshot.f32_bits.len() {
            return Err(format!(
                "compact replay segment {segment_index} has an out-of-range f32 slot {}",
                change.slot
            ));
        }
        if previous_slot.is_some_and(|previous| change.slot <= previous) {
            return Err(format!(
                "compact replay segment {segment_index} f32 changes must be sorted and unique"
            ));
        }
        if snapshot.f32_bits[change.slot] == change.bits {
            return Err(format!(
                "compact replay segment {segment_index} f32 change at slot {} is redundant",
                change.slot
            ));
        }
        previous_slot = Some(change.slot);
    }
    for change in &segment.i32_changes {
        snapshot.i32_values[change.slot] = change.value;
    }
    for change in &segment.f32_changes {
        snapshot.f32_bits[change.slot] = change.bits;
    }
    Ok(())
}

#[cfg(test)]
fn compact_input_at_tick(
    document: &CompactReplayDocument,
    tick: u64,
) -> Result<CompactInputSnapshot, String> {
    validate_compact_document(document)?;
    if tick == 0 || tick > document.total_ticks {
        return Err(format!(
            "compact replay tick {tick} is outside 1..={}",
            document.total_ticks
        ));
    }
    let mut snapshot = CompactInputSnapshot {
        i32_values: document.initial_input.i32_values.clone(),
        f32_bits: document.initial_input.f32_bits.clone(),
    };
    let mut cursor = 0u64;
    for segment in &document.segments {
        let gap_end = cursor + segment.tick_gap;
        if tick <= gap_end {
            return Ok(snapshot);
        }
        for change in &segment.i32_changes {
            snapshot.i32_values[change.slot] = change.value;
        }
        for change in &segment.f32_changes {
            snapshot.f32_bits[change.slot] = change.bits;
        }
        let run_end = gap_end + segment.run_ticks;
        if tick <= run_end {
            return Ok(snapshot);
        }
        cursor = run_end;
    }
    Err(format!("compact replay has no segment for tick {tick}"))
}

/// Batch-only helper for codec tests; runtime recording will use a streaming
/// cursor once compact replay is wired into the recording path.
#[cfg(test)]
fn build_compact_segments(
    baseline: &CompactInputSnapshot,
    ticks: &[CompactInputSnapshot],
) -> Result<Vec<CompactInputSegment>, String> {
    if ticks.is_empty() {
        return Err("compact replay requires at least one tick".to_string());
    }
    if ticks.len() > MAX_REPLAY_FRAMES {
        return Err(format!(
            "compact replay batch exceeds the {MAX_REPLAY_FRAMES}-tick limit"
        ));
    }
    if baseline.i32_values.len() > MAX_HOST_FRAME_VALUES
        || baseline.f32_bits.len() > MAX_HOST_FRAME_VALUES
        || baseline.i32_values.len() != ticks[0].i32_values.len()
        || baseline.f32_bits.len() != ticks[0].f32_bits.len()
    {
        return Err("compact replay baseline dimensions do not match ticks".to_string());
    }
    let mut segments = Vec::new();
    let mut previous = baseline.clone();
    let mut start = 0usize;
    while start < ticks.len() {
        let current = &ticks[start];
        if current.i32_values.len() != baseline.i32_values.len()
            || current.f32_bits.len() != baseline.f32_bits.len()
        {
            return Err(format!("compact replay tick {start} dimensions changed"));
        }
        let mut end = start + 1;
        while end < ticks.len() && ticks[end] == *current {
            end += 1;
        }
        let i32_changes = previous
            .i32_values
            .iter()
            .zip(&current.i32_values)
            .enumerate()
            .filter_map(|(slot, (previous, current))| {
                (previous != current).then_some(CompactI32Change {
                    slot,
                    value: *current,
                })
            })
            .collect();
        let f32_changes = previous
            .f32_bits
            .iter()
            .zip(&current.f32_bits)
            .enumerate()
            .filter_map(|(slot, (previous, current))| {
                (previous != current).then_some(CompactF32Change {
                    slot,
                    bits: *current,
                })
            })
            .collect();
        segments.push(CompactInputSegment {
            tick_gap: 0,
            run_ticks: (end - start) as u64,
            i32_changes,
            f32_changes,
        });
        previous = current.clone();
        start = end;
    }
    Ok(segments)
}

struct LegacyReplayRecorder {
    document: ReplayDocument,
    previous_i32: Vec<i32>,
    previous_f32: Vec<f32>,
}

struct CompactPendingTick {
    tick: u64,
    input: CompactInputSnapshot,
}

struct CompactReplayRecorder {
    document: CompactReplayDocument,
    encoded_bytes: usize,
    previous_input: Option<CompactInputSnapshot>,
    active_snapshot: Option<CompactInputSnapshot>,
    active_segment: Option<CompactInputSegment>,
    pending: Option<CompactPendingTick>,
}

pub(crate) struct ReplayRecorder {
    output: PathBuf,
    legacy: Option<LegacyReplayRecorder>,
    compact: Option<CompactReplayRecorder>,
}

impl ReplayRecorder {
    /// Start the schema-v1 recorder retained for existing callers.
    ///
    /// New integrations should use `start_compact` after their whole-game
    /// input-use pass has produced the observed descriptors.
    #[cfg(test)]
    pub(crate) fn start(
        output: PathBuf,
        jit: &JitProcess,
        host_i32_count: usize,
        host_f32_count: usize,
    ) -> Result<Self, String> {
        prepare_recording_output(&output)?;
        let document = ReplayDocument {
            schema_version: REPLAY_SCHEMA_VERSION,
            identity: replay_identity(jit, host_i32_count, host_f32_count)?,
            initial_state: capture_initial_state(jit)?,
            frames: Vec::new(),
        };
        Ok(Self {
            output,
            legacy: Some(LegacyReplayRecorder {
                document,
                previous_i32: vec![0; host_i32_count],
                previous_f32: vec![0.0; host_f32_count],
            }),
            compact: None,
        })
    }

    /// Start bounded schema-v2 recording from caller-supplied observed input
    /// descriptors. The descriptors are the already-computed whole-game union;
    /// this module only validates, projects, and records their slots.
    pub(crate) fn start_compact(
        output: PathBuf,
        jit: &JitProcess,
        host_i32_count: usize,
        host_f32_count: usize,
        observed_i32: &[CompactI32Field],
        observed_f32: &[CompactF32Field],
        metadata: CompactReplayMetadata,
    ) -> Result<Self, String> {
        validate_compact_replay_contract(jit)?;
        prepare_recording_output(&output)?;
        if host_i32_count == 0
            || host_f32_count == 0
            || host_i32_count > MAX_HOST_FRAME_VALUES
            || host_f32_count > MAX_HOST_FRAME_VALUES
        {
            return Err(format!(
                "compact replay HostFrame dimensions must be between 1 and {MAX_HOST_FRAME_VALUES}"
            ));
        }
        validate_compact_i32_fields(observed_i32, host_i32_count)?;
        validate_compact_f32_fields(observed_f32, host_f32_count)?;
        validate_compact_metadata(&metadata)?;
        let base = replay_identity(jit, host_i32_count, host_f32_count)?;
        let compiler_layout_sha256 = compact_compiler_layout_identity(jit)?;
        let identity = CompactReplayIdentity {
            stasis_version: base.stasis_version,
            release_id: base.release_id,
            target: base.target,
            source_sha256: base.source_sha256,
            state_layout_sha256: base.state_layout_sha256,
            compiler_layout_sha256,
            runtime_sha256: metadata.runtime_sha256,
            asset_manifest_sha256: metadata.asset_manifest_sha256,
            host_schema_version: metadata.host_schema_version,
            host_i32_count,
            host_f32_count,
            input_usage_sha256: compact_input_usage_hash(observed_i32, observed_f32),
            tick_rate_hz: metadata.tick_rate_hz,
            hash_scope: CompactHashScope::SimulationAfterTick,
            determinism_profile: metadata.determinism_profile,
            controller_schema_version: metadata.controller_schema_version,
            observed_i32: observed_i32.to_vec(),
            observed_f32: observed_f32.to_vec(),
        };
        let document = CompactReplayDocument {
            schema_version: COMPACT_REPLAY_SCHEMA_VERSION,
            identity,
            initial_state: capture_initial_state(jit)?,
            initial_input: CompactInputBaseline {
                i32_values: Vec::new(),
                f32_bits: Vec::new(),
            },
            segments: Vec::new(),
            checkpoints: Vec::new(),
            total_ticks: 0,
            final_state: CompactReplayFinal {
                tick: 0,
                state_sha256: String::new(),
            },
        };
        let encoded_bytes = serde_json::to_vec(&document)
            .map_err(|error| format!("failed to size compact replay header: {error}"))?
            .len()
            .checked_add(4_096)
            .ok_or_else(|| "compact replay size estimate overflowed".to_string())?;
        if encoded_bytes > MAX_REPLAY_FILE_BYTES as usize {
            return Err(format!(
                "compact replay header exceeds the {MAX_REPLAY_FILE_BYTES}-byte limit"
            ));
        }
        Ok(Self {
            output,
            legacy: None,
            compact: Some(CompactReplayRecorder {
                document,
                encoded_bytes,
                previous_input: None,
                active_snapshot: None,
                active_segment: None,
                pending: None,
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn start_compact_with_defaults(
        output: PathBuf,
        jit: &JitProcess,
        host_i32_count: usize,
        host_f32_count: usize,
        observed_i32: &[CompactI32Field],
        observed_f32: &[CompactF32Field],
    ) -> Result<Self, String> {
        Self::start_compact(
            output,
            jit,
            host_i32_count,
            host_f32_count,
            observed_i32,
            observed_f32,
            CompactReplayMetadata::default(),
        )
    }

    pub(crate) fn begin_tick(
        &mut self,
        tick: u64,
        host_i32: &[i32],
        host_f32: &[f32],
    ) -> Result<(), String> {
        if let Some(recorder) = self.legacy.as_mut() {
            if recorder.document.frames.len() >= MAX_REPLAY_FRAMES {
                return Err(format!(
                    "replay recording exceeds the {MAX_REPLAY_FRAMES}-frame limit"
                ));
            }
            if recorder
                .document
                .frames
                .last()
                .is_some_and(|frame| frame.state_sha256.is_empty())
            {
                return Err(format!("replay tick {tick} has not been completed"));
            }
            if tick != recorder.document.frames.len() as u64 + 1 {
                return Err(format!(
                    "replay tick sequence mismatch: expected {}, found {tick}",
                    recorder.document.frames.len() + 1
                ));
            }
            if host_i32.len() != recorder.previous_i32.len()
                || host_f32.len() != recorder.previous_f32.len()
            {
                return Err("HostFrame size changed while recording replay".to_string());
            }
            recorder.document.frames.push(ReplayFrame {
                tick,
                i32_changes: diff_i32(&recorder.previous_i32, host_i32),
                f32_changes: diff_f32(&recorder.previous_f32, host_f32),
                state_sha256: String::new(),
            });
            recorder.previous_i32.copy_from_slice(host_i32);
            recorder.previous_f32.copy_from_slice(host_f32);
            return Ok(());
        }

        let recorder = self
            .compact
            .as_mut()
            .ok_or_else(|| "replay recorder has no recording mode".to_string())?;
        if recorder.document.total_ticks >= MAX_REPLAY_FRAMES as u64 {
            return Err(format!(
                "compact replay recording exceeds the {MAX_REPLAY_FRAMES}-tick limit"
            ));
        }
        if recorder.pending.is_some() {
            return Err(format!("replay tick {tick} has not been completed"));
        }
        let expected = recorder
            .document
            .total_ticks
            .checked_add(1)
            .ok_or_else(|| "compact replay tick counter overflowed".to_string())?;
        if tick != expected {
            return Err(format!(
                "compact replay tick sequence mismatch: expected {expected}, found {tick}"
            ));
        }
        let input = compact_snapshot_from_host(&recorder.document.identity, host_i32, host_f32)?;
        recorder.pending = Some(CompactPendingTick { tick, input });
        Ok(())
    }

    pub(crate) fn finish_tick(&mut self, jit: &JitProcess) -> Result<(), String> {
        if let Some(recorder) = self.legacy.as_mut() {
            let frame = recorder
                .document
                .frames
                .last_mut()
                .ok_or_else(|| "replay recorder has no active tick".to_string())?;
            if !frame.state_sha256.is_empty() {
                return Err(format!("replay tick {} was already completed", frame.tick));
            }
            frame.state_sha256 = simulation_state_hash(jit)?;
            return Ok(());
        }
        let recorder = self
            .compact
            .as_mut()
            .ok_or_else(|| "replay recorder has no recording mode".to_string())?;
        let pending = recorder
            .pending
            .take()
            .ok_or_else(|| "replay recorder has no active tick".to_string())?;
        let state_sha256 = simulation_state_hash(jit)?;
        let tick = pending.tick;
        let input = pending.input;
        if recorder.previous_input.is_none() {
            recorder.document.initial_input = CompactInputBaseline {
                i32_values: input.i32_values.clone(),
                f32_bits: input.f32_bits.clone(),
            };
            let baseline_bytes = serde_json::to_vec(&recorder.document.initial_input)
                .map_err(|error| format!("failed to size compact replay baseline: {error}"))?
                .len();
            reserve_compact_bytes(recorder, baseline_bytes, "input baseline")?;
        }
        append_compact_input(recorder, input)?;
        recorder.document.total_ticks = tick;
        recorder.document.final_state = CompactReplayFinal {
            tick,
            state_sha256: state_sha256.clone(),
        };
        if tick % COMPACT_CHECKPOINT_INTERVAL == 0 {
            if recorder.document.checkpoints.len() >= MAX_COMPACT_CHECKPOINTS {
                return Err(format!(
                    "compact replay exceeds the {MAX_COMPACT_CHECKPOINTS}-checkpoint limit"
                ));
            }
            let checkpoint = CompactReplayCheckpoint { tick, state_sha256 };
            let checkpoint_bytes = serde_json::to_vec(&checkpoint)
                .map_err(|error| format!("failed to size compact replay checkpoint: {error}"))?
                .len();
            reserve_compact_bytes(recorder, checkpoint_bytes + 1, "checkpoint")?;
            recorder.document.checkpoints.push(checkpoint);
        }
        Ok(())
    }

    pub(crate) fn discard_tick(&mut self, tick: u64) -> Result<(), String> {
        if let Some(recorder) = self.legacy.as_mut() {
            let frame = recorder
                .document
                .frames
                .last()
                .ok_or_else(|| format!("replay has no active tick {tick}"))?;
            if frame.tick != tick || !frame.state_sha256.is_empty() {
                return Err(format!(
                    "replay tick {tick} is not an unfinished active tick"
                ));
            }
            recorder.document.frames.pop();
            return Ok(());
        }
        let recorder = self
            .compact
            .as_mut()
            .ok_or_else(|| "replay recorder has no recording mode".to_string())?;
        if recorder
            .pending
            .as_ref()
            .is_some_and(|pending| pending.tick == tick)
        {
            recorder.pending = None;
            Ok(())
        } else {
            Err(format!(
                "compact replay tick {tick} is not an unfinished active tick"
            ))
        }
    }

    pub(crate) fn publish(mut self) -> Result<PathBuf, String> {
        if let Some(recorder) = self.legacy.take() {
            if recorder.document.frames.is_empty() {
                return Err("cannot publish a replay without a completed tick".to_string());
            }
            if recorder
                .document
                .frames
                .last()
                .is_some_and(|frame| frame.state_sha256.is_empty())
            {
                return Err("cannot publish an incomplete replay tick".to_string());
            }
            let bytes = serde_json::to_vec(&recorder.document)
                .map_err(|error| format!("failed to encode replay recording: {error}"))?;
            return publish_replay_bytes(&self.output, &bytes);
        }
        let mut recorder = self
            .compact
            .take()
            .ok_or_else(|| "replay recorder has no recording mode".to_string())?;
        if recorder.pending.is_some() {
            return Err("cannot publish an incomplete replay tick".to_string());
        }
        flush_compact_segment(&mut recorder)?;
        if recorder.document.total_ticks == 0 {
            return Err("cannot publish a replay without a completed tick".to_string());
        }
        let bytes = recorder.document.canonical_bytes()?;
        publish_replay_bytes(&self.output, &bytes)
    }
}

fn prepare_recording_output(output: &Path) -> Result<(), String> {
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
    })
}

fn publish_replay_bytes(output: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() as u64 > MAX_REPLAY_FILE_BYTES {
        return Err(format!(
            "replay recording is too large ({} bytes; maximum {MAX_REPLAY_FILE_BYTES})",
            bytes.len()
        ));
    }
    let temporary = output.with_extension(format!(
        "{}.tmp",
        output
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
        file.write_all(bytes).map_err(|error| {
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
        stasis_dynload::atomic_rename_no_replace(&temporary, output).map_err(|error| {
            format!(
                "failed to publish replay recording {}: {error}",
                output.display()
            )
        })
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    Ok(output.to_path_buf())
}

fn validate_compact_metadata(metadata: &CompactReplayMetadata) -> Result<(), String> {
    validate_compact_hash("runtime_sha256", &metadata.runtime_sha256)?;
    if let Some(hash) = metadata.asset_manifest_sha256.as_deref() {
        validate_compact_hash("asset_manifest_sha256", hash)?;
    }
    if metadata.host_schema_version == 0 {
        return Err("compact replay host_schema_version must be positive".to_string());
    }
    if metadata.tick_rate_hz == 0 {
        return Err("compact replay tick_rate_hz must be positive".to_string());
    }
    validate_compact_field_text("determinism_profile", &metadata.determinism_profile)?;
    if metadata
        .controller_schema_version
        .is_some_and(|version| version == 0)
    {
        return Err(
            "compact replay controller_schema_version must be positive when present".to_string(),
        );
    }
    Ok(())
}

fn compact_input_usage_hash(
    observed_i32: &[CompactI32Field],
    observed_f32: &[CompactF32Field],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"stasis.replay.input-usage.v1\0");
    hasher.update([b'i']);
    hasher.update((observed_i32.len() as u64).to_le_bytes());
    for field in observed_i32 {
        hasher.update((field.slot as u64).to_le_bytes());
        hasher.update((field.index as u64).to_le_bytes());
        hasher.update((field.path.len() as u64).to_le_bytes());
        hasher.update(field.path.as_bytes());
        hasher.update((field.family.len() as u64).to_le_bytes());
        hasher.update(field.family.as_bytes());
    }
    hasher.update([b'f']);
    hasher.update((observed_f32.len() as u64).to_le_bytes());
    for field in observed_f32 {
        hasher.update((field.slot as u64).to_le_bytes());
        hasher.update((field.index as u64).to_le_bytes());
        hasher.update((field.path.len() as u64).to_le_bytes());
        hasher.update(field.path.as_bytes());
        hasher.update((field.family.len() as u64).to_le_bytes());
        hasher.update(field.family.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn compact_snapshot_from_host(
    identity: &CompactReplayIdentity,
    host_i32: &[i32],
    host_f32: &[f32],
) -> Result<CompactInputSnapshot, String> {
    if host_i32.len() != identity.host_i32_count || host_f32.len() != identity.host_f32_count {
        return Err("HostFrame size changed while recording compact replay".to_string());
    }
    let i32_values = identity
        .observed_i32
        .iter()
        .map(|field| {
            host_i32
                .get(field.index)
                .copied()
                .ok_or_else(|| format!("compact replay i32 index {} is out of range", field.index))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let f32_bits = identity
        .observed_f32
        .iter()
        .map(|field| {
            host_f32
                .get(field.index)
                .map(|value| value.to_bits())
                .ok_or_else(|| format!("compact replay f32 index {} is out of range", field.index))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CompactInputSnapshot {
        i32_values,
        f32_bits,
    })
}

fn compact_diff(
    previous: &CompactInputSnapshot,
    current: &CompactInputSnapshot,
) -> (Vec<CompactI32Change>, Vec<CompactF32Change>) {
    let i32_changes = previous
        .i32_values
        .iter()
        .zip(&current.i32_values)
        .enumerate()
        .filter_map(|(slot, (previous, current))| {
            (previous != current).then_some(CompactI32Change {
                slot,
                value: *current,
            })
        })
        .collect();
    let f32_changes = previous
        .f32_bits
        .iter()
        .zip(&current.f32_bits)
        .enumerate()
        .filter_map(|(slot, (previous, current))| {
            (previous != current).then_some(CompactF32Change {
                slot,
                bits: *current,
            })
        })
        .collect();
    (i32_changes, f32_changes)
}

fn append_compact_input(
    recorder: &mut CompactReplayRecorder,
    input: CompactInputSnapshot,
) -> Result<(), String> {
    if let Some(active_snapshot) = recorder.active_snapshot.as_ref() {
        if *active_snapshot == input {
            let active_segment = recorder
                .active_segment
                .as_mut()
                .ok_or_else(|| "compact replay active snapshot has no segment".to_string())?;
            active_segment.run_ticks = active_segment
                .run_ticks
                .checked_add(1)
                .ok_or_else(|| "compact replay run length overflowed".to_string())?;
            recorder.previous_input.get_or_insert_with(|| input.clone());
            return Ok(());
        }
    }
    flush_compact_segment(recorder)?;
    let changes = recorder
        .previous_input
        .as_ref()
        .map(|previous| compact_diff(previous, &input))
        .unwrap_or_else(|| (Vec::new(), Vec::new()));
    recorder.active_segment = Some(CompactInputSegment {
        tick_gap: 0,
        run_ticks: 1,
        i32_changes: changes.0,
        f32_changes: changes.1,
    });
    recorder.active_snapshot = Some(input.clone());
    recorder.previous_input = Some(input);
    Ok(())
}

fn flush_compact_segment(recorder: &mut CompactReplayRecorder) -> Result<(), String> {
    let Some(segment) = recorder.active_segment.take() else {
        return Ok(());
    };
    recorder.active_snapshot = None;
    if recorder.document.segments.len() >= MAX_REPLAY_FRAMES {
        return Err(format!(
            "compact replay exceeds the {MAX_REPLAY_FRAMES}-segment limit"
        ));
    }
    let segment_bytes = serde_json::to_vec(&segment)
        .map_err(|error| format!("failed to size compact replay segment: {error}"))?
        .len();
    reserve_compact_bytes(recorder, segment_bytes + 1, "input segment")?;
    recorder.document.segments.push(segment);
    Ok(())
}

fn reserve_compact_bytes(
    recorder: &mut CompactReplayRecorder,
    additional: usize,
    label: &str,
) -> Result<(), String> {
    let next = recorder
        .encoded_bytes
        .checked_add(additional)
        .ok_or_else(|| "compact replay size estimate overflowed".to_string())?;
    if next > MAX_REPLAY_FILE_BYTES as usize {
        return Err(format!(
            "compact replay {label} exceeds the {MAX_REPLAY_FILE_BYTES}-byte limit"
        ));
    }
    recorder.encoded_bytes = next;
    Ok(())
}

enum ReplayPlaybackDocument {
    Legacy(ReplayDocument),
    Compact(CompactReplayDocument),
}

pub(crate) struct ReplayPlayer {
    document: ReplayPlaybackDocument,
    next_frame: usize,
    next_tick: u64,
    host_i32: Vec<i32>,
    host_f32: Vec<f32>,
    compact_segment_index: usize,
    compact_segment_cursor: u64,
    compact_segment_applied: bool,
    compact_snapshot: Option<CompactInputSnapshot>,
    next_checkpoint: usize,
    last_verified_tick: u64,
    first_divergence: Option<String>,
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
        let document = match decode_versioned_replay(&source)
            .map_err(|error| format!("failed to parse replay {}: {error}", path.display()))?
        {
            VersionedReplayDocument::LegacyV1(document) => ReplayPlaybackDocument::Legacy(document),
            VersionedReplayDocument::CompactV2(document) => {
                ReplayPlaybackDocument::Compact(document)
            }
        };
        let (i32_count, f32_count, compact_snapshot) = match &document {
            ReplayPlaybackDocument::Legacy(document) => (
                document.identity.host_i32_count,
                document.identity.host_f32_count,
                None,
            ),
            ReplayPlaybackDocument::Compact(document) => (
                document.identity.host_i32_count,
                document.identity.host_f32_count,
                Some(CompactInputSnapshot {
                    i32_values: document.initial_input.i32_values.clone(),
                    f32_bits: document.initial_input.f32_bits.clone(),
                }),
            ),
        };
        Ok(Self {
            document,
            next_frame: 0,
            next_tick: 1,
            host_i32: vec![0; i32_count],
            host_f32: vec![0.0; f32_count],
            compact_segment_index: 0,
            compact_segment_cursor: 0,
            compact_segment_applied: false,
            compact_snapshot,
            next_checkpoint: 0,
            last_verified_tick: 0,
            first_divergence: None,
        })
    }

    pub(crate) fn frame_count(&self) -> u64 {
        match &self.document {
            ReplayPlaybackDocument::Legacy(document) => document.frames.len() as u64,
            ReplayPlaybackDocument::Compact(document) => document.total_ticks,
        }
    }

    pub(crate) fn is_compact(&self) -> bool {
        matches!(self.document, ReplayPlaybackDocument::Compact(_))
    }

    pub(crate) fn initialize_with_metadata(
        &self,
        jit: &JitProcess,
        active_host_i32_count: usize,
        active_host_f32_count: usize,
        metadata: &CompactReplayMetadata,
    ) -> Result<(), String> {
        if self.is_compact() {
            validate_compact_replay_contract(jit)?;
        }
        let (identity, initial_state) = match &self.document {
            ReplayPlaybackDocument::Legacy(document) => (
                ReplayIdentityView::Legacy(&document.identity),
                &document.initial_state,
            ),
            ReplayPlaybackDocument::Compact(document) => (
                ReplayIdentityView::Compact(&document.identity),
                &document.initial_state,
            ),
        };
        let actual = replay_identity(jit, active_host_i32_count, active_host_f32_count)?;
        if let Some(reason) = identity.mismatch(&actual, jit, metadata)? {
            return Err(format!("replay identity mismatch: {reason}"));
        }
        restore_initial_state(jit, initial_state)?;
        let actual_hash = simulation_state_hash(jit)?;
        if actual_hash != initial_state.state_sha256 {
            return Err(format!(
                "replay initial state mismatch: expected {}, found {actual_hash}",
                initial_state.state_sha256
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn initialize(&self, jit: &JitProcess) -> Result<(), String> {
        self.initialize_with_metadata(
            jit,
            self.host_i32.len(),
            self.host_f32.len(),
            &CompactReplayMetadata::default(),
        )
    }

    pub(crate) fn apply_next(
        &mut self,
        tick: u64,
        host_i32: &mut [i32],
        host_f32: &mut [f32],
    ) -> Result<(), String> {
        if host_i32.len() != self.host_i32.len() || host_f32.len() != self.host_f32.len() {
            return Err("active HostFrame size does not match replay".to_string());
        }
        match &mut self.document {
            ReplayPlaybackDocument::Legacy(document) => {
                let frame = document
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
                self.next_frame += 1;
            }
            ReplayPlaybackDocument::Compact(document) => {
                if tick != self.next_tick {
                    return Err(format!(
                        "compact replay tick sequence mismatch: expected {}, found {tick}",
                        self.next_tick
                    ));
                }
                if tick > document.total_ticks {
                    return Err(format!(
                        "compact replay has no frame for tick {tick} (total {})",
                        document.total_ticks
                    ));
                }
                let snapshot = self
                    .compact_snapshot
                    .as_mut()
                    .ok_or_else(|| "compact replay has no input baseline".to_string())?;
                advance_compact_snapshot(
                    document,
                    tick,
                    &mut self.compact_segment_index,
                    &mut self.compact_segment_cursor,
                    &mut self.compact_segment_applied,
                    snapshot,
                )?;
                self.host_i32.fill(0);
                self.host_f32.fill(0.0);
                apply_compact_snapshot(
                    &document.identity,
                    snapshot,
                    &mut self.host_i32,
                    &mut self.host_f32,
                )?;
                self.next_tick = self
                    .next_tick
                    .checked_add(1)
                    .ok_or_else(|| "compact replay tick counter overflowed".to_string())?;
            }
        }
        host_i32.copy_from_slice(&self.host_i32);
        host_f32.copy_from_slice(&self.host_f32);
        Ok(())
    }

    pub(crate) fn verify_tick(&mut self, tick: u64, jit: &JitProcess) -> Result<(), String> {
        if let Some(error) = self.first_divergence.as_deref() {
            return Err(error.to_string());
        }
        match &self.document {
            ReplayPlaybackDocument::Legacy(document) => {
                let frame = document
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
            ReplayPlaybackDocument::Compact(document) => {
                let completed_tick = self.next_tick.saturating_sub(1);
                if tick != completed_tick || tick == 0 {
                    return Err(format!(
                        "compact replay verification sequence mismatch: expected {completed_tick}, found {tick}"
                    ));
                }
                let expected =
                    compact_expected_checkpoint(document, tick, &mut self.next_checkpoint);
                let Some(expected) = expected else {
                    return Ok(());
                };
                let actual = simulation_state_hash(jit)?;
                if actual != expected {
                    let interval_start = self.last_verified_tick.saturating_add(1);
                    let mut diagnostic = format!(
                        "replay diverged within ticks {interval_start}..={tick}; detected at checkpoint {tick}: expected state {expected}, found {actual}"
                    );
                    diagnostic.truncate(MAX_COMPACT_DIAGNOSTIC_BYTES);
                    self.first_divergence = Some(diagnostic.clone());
                    return Err(diagnostic);
                }
                self.last_verified_tick = tick;
                Ok(())
            }
        }
    }
}

enum ReplayIdentityView<'a> {
    Legacy(&'a ReplayIdentity),
    Compact(&'a CompactReplayIdentity),
}

impl ReplayIdentityView<'_> {
    fn mismatch(
        &self,
        actual: &ReplayIdentity,
        jit: &JitProcess,
        metadata: &CompactReplayMetadata,
    ) -> Result<Option<String>, String> {
        match self {
            Self::Legacy(identity) => Ok((identity.stasis_version != actual.stasis_version
                || identity.release_id != actual.release_id
                || identity.target != actual.target
                || identity.source_sha256 != actual.source_sha256
                || identity.state_layout_sha256 != actual.state_layout_sha256
                || identity.host_i32_count != actual.host_i32_count
                || identity.host_f32_count != actual.host_f32_count)
                .then(|| "legacy source, build, target, or state layout differs".to_string())),
            Self::Compact(identity) => {
                let (observed_i32, observed_f32) = compact_observed_fields(jit)?;
                let reason = if identity.stasis_version != actual.stasis_version {
                    Some("Stasis version differs")
                } else if identity.release_id != actual.release_id {
                    Some("release identity differs")
                } else if identity.target != actual.target {
                    Some("target differs")
                } else if identity.source_sha256 != actual.source_sha256 {
                    Some("source hash differs")
                } else if identity.state_layout_sha256 != actual.state_layout_sha256 {
                    Some("state layout hash differs")
                } else if identity.compiler_layout_sha256 != compact_compiler_layout_identity(jit)?
                {
                    Some("compiler layout identity differs")
                } else if identity.host_i32_count != actual.host_i32_count
                    || identity.host_f32_count != actual.host_f32_count
                {
                    Some("HostFrame dimensions differ")
                } else if identity.runtime_sha256 != metadata.runtime_sha256 {
                    Some("runtime identity differs")
                } else if identity.asset_manifest_sha256 != metadata.asset_manifest_sha256 {
                    Some("asset manifest identity differs")
                } else if identity.host_schema_version != metadata.host_schema_version {
                    Some("HostFrame schema differs")
                } else if identity.tick_rate_hz != metadata.tick_rate_hz {
                    Some("tick rate differs")
                } else if identity.hash_scope != CompactHashScope::SimulationAfterTick {
                    Some("verification hash scope differs")
                } else if identity.determinism_profile != metadata.determinism_profile {
                    Some("determinism profile differs")
                } else if identity.controller_schema_version != metadata.controller_schema_version {
                    Some("controller schema differs")
                } else if identity.input_usage_sha256
                    != compact_input_usage_hash(&observed_i32, &observed_f32)
                    || identity.observed_i32 != observed_i32
                    || identity.observed_f32 != observed_f32
                {
                    Some("compiler-observed input usage differs")
                } else {
                    None
                };
                Ok(reason.map(str::to_string))
            }
        }
    }
}

fn advance_compact_snapshot(
    document: &CompactReplayDocument,
    tick: u64,
    segment_index: &mut usize,
    segment_cursor: &mut u64,
    segment_applied: &mut bool,
    snapshot: &mut CompactInputSnapshot,
) -> Result<(), String> {
    loop {
        let segment = document
            .segments
            .get(*segment_index)
            .ok_or_else(|| format!("compact replay has no segment for tick {tick}"))?;
        let gap_end = segment_cursor
            .checked_add(segment.tick_gap)
            .ok_or_else(|| "compact replay tick gap overflows during playback".to_string())?;
        if tick <= gap_end {
            return Ok(());
        }
        let run_end = gap_end
            .checked_add(segment.run_ticks)
            .ok_or_else(|| "compact replay run length overflows during playback".to_string())?;
        if tick <= run_end {
            if !*segment_applied {
                for change in &segment.i32_changes {
                    snapshot.i32_values[change.slot] = change.value;
                }
                for change in &segment.f32_changes {
                    snapshot.f32_bits[change.slot] = change.bits;
                }
                *segment_applied = true;
            }
            return Ok(());
        }
        *segment_index += 1;
        *segment_cursor = run_end;
        *segment_applied = false;
    }
}

fn apply_compact_snapshot(
    identity: &CompactReplayIdentity,
    snapshot: &CompactInputSnapshot,
    host_i32: &mut [i32],
    host_f32: &mut [f32],
) -> Result<(), String> {
    for field in &identity.observed_i32 {
        *host_i32
            .get_mut(field.index)
            .ok_or_else(|| format!("compact replay i32 index {} is out of range", field.index))? =
            snapshot.i32_values[field.slot];
    }
    for field in &identity.observed_f32 {
        *host_f32
            .get_mut(field.index)
            .ok_or_else(|| format!("compact replay f32 index {} is out of range", field.index))? =
            f32::from_bits(snapshot.f32_bits[field.slot]);
    }
    Ok(())
}

fn compact_expected_checkpoint<'a>(
    document: &'a CompactReplayDocument,
    tick: u64,
    next_checkpoint: &mut usize,
) -> Option<&'a str> {
    while document
        .checkpoints
        .get(*next_checkpoint)
        .is_some_and(|checkpoint| checkpoint.tick < tick)
    {
        *next_checkpoint += 1;
    }
    if tick == document.final_state.tick {
        while document
            .checkpoints
            .get(*next_checkpoint)
            .is_some_and(|checkpoint| checkpoint.tick <= tick)
        {
            *next_checkpoint += 1;
        }
        return Some(&document.final_state.state_sha256);
    }
    if document
        .checkpoints
        .get(*next_checkpoint)
        .is_some_and(|checkpoint| checkpoint.tick == tick)
    {
        let checkpoint = &document.checkpoints[*next_checkpoint];
        *next_checkpoint += 1;
        Some(&checkpoint.state_sha256)
    } else {
        None
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

    fn compact_test_identity(i32_count: usize, f32_count: usize) -> CompactReplayIdentity {
        let observed_i32 = (0..i32_count)
            .map(|slot| CompactI32Field {
                slot,
                index: slot,
                path: format!("keyboard.values[{slot}]"),
                family: "keyboard".to_string(),
            })
            .collect::<Vec<_>>();
        let observed_f32 = (0..f32_count)
            .map(|slot| CompactF32Field {
                slot,
                index: slot,
                path: format!("pointer.values[{slot}]"),
                family: "pointer".to_string(),
            })
            .collect::<Vec<_>>();
        CompactReplayIdentity {
            stasis_version: "test".to_string(),
            release_id: "test".to_string(),
            target: "test".to_string(),
            source_sha256: "0".repeat(64),
            state_layout_sha256: "0".repeat(64),
            compiler_layout_sha256: "0".repeat(64),
            runtime_sha256: "0".repeat(64),
            asset_manifest_sha256: None,
            host_schema_version: HOST_FRAME_SCHEMA_VERSION,
            host_i32_count: i32_count.max(1),
            host_f32_count: f32_count.max(1),
            input_usage_sha256: compact_input_usage_hash(&observed_i32, &observed_f32),
            tick_rate_hz: 60,
            hash_scope: CompactHashScope::SimulationAfterTick,
            determinism_profile: "input_only_no_external_observations".to_string(),
            controller_schema_version: None,
            observed_i32,
            observed_f32,
        }
    }

    fn compact_test_document(
        baseline: CompactInputSnapshot,
        segments: Vec<CompactInputSegment>,
        total_ticks: u64,
    ) -> CompactReplayDocument {
        let checkpoints = (1..=total_ticks / COMPACT_CHECKPOINT_INTERVAL)
            .map(|checkpoint| CompactReplayCheckpoint {
                tick: checkpoint * COMPACT_CHECKPOINT_INTERVAL,
                state_sha256: "0".repeat(64),
            })
            .collect();
        CompactReplayDocument {
            schema_version: COMPACT_REPLAY_SCHEMA_VERSION,
            identity: compact_test_identity(baseline.i32_values.len(), baseline.f32_bits.len()),
            initial_state: InitialState {
                values: Vec::new(),
                state_sha256: "0".repeat(64),
            },
            initial_input: CompactInputBaseline {
                i32_values: baseline.i32_values,
                f32_bits: baseline.f32_bits,
            },
            segments,
            checkpoints,
            total_ticks,
            final_state: CompactReplayFinal {
                tick: total_ticks,
                state_sha256: "0".repeat(64),
            },
        }
    }

    fn record_compact_identity_fixture(
        output: &Path,
        metadata: CompactReplayMetadata,
    ) -> JitProcess {
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "main.stasis",
            "global score: i32; \
             function main(): i32 { score = 0; return 0; } \
             function tick(): i32 { score += 1; return 0; } \
             function render(): i32 { return 0; }",
        );
        jit.compile().expect("compile identity fixture");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

        let mut recorder =
            ReplayRecorder::start_compact(output.to_path_buf(), &jit, 1, 1, &[], &[], metadata)
                .expect("start compact identity fixture");
        recorder
            .begin_tick(1, &[0], &[0.0])
            .expect("begin identity fixture tick");
        assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
        recorder
            .finish_tick(&jit)
            .expect("finish identity fixture tick");
        recorder.publish().expect("publish identity fixture");
        jit
    }

    #[test]
    fn compact_replay_rejects_active_runtime_hash_mismatch_with_field_specific_diagnostic() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "stasis-compact-runtime-identity-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("runtime.replay.json");
        let mut recorded = CompactReplayMetadata::default();
        recorded.runtime_sha256 = "1".repeat(64);
        let jit = record_compact_identity_fixture(&path, recorded.clone());

        let mut active = recorded;
        active.runtime_sha256 = "2".repeat(64);
        let player = ReplayPlayer::load(&path).expect("load identity fixture");
        let error = player
            .initialize_with_metadata(&jit, 1, 1, &active)
            .expect_err("active runtime hash mismatch must be rejected");
        assert_eq!(error, "replay identity mismatch: runtime identity differs");
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn compact_replay_rejects_active_asset_manifest_hash_mismatch_with_field_specific_diagnostic() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "stasis-compact-asset-identity-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join("asset.replay.json");
        let mut recorded = CompactReplayMetadata::default();
        recorded.runtime_sha256 = "1".repeat(64);
        recorded.asset_manifest_sha256 = Some("2".repeat(64));
        let jit = record_compact_identity_fixture(&path, recorded.clone());

        let mut active = recorded;
        active.asset_manifest_sha256 = Some("3".repeat(64));
        let player = ReplayPlayer::load(&path).expect("load identity fixture");
        let error = player
            .initialize_with_metadata(&jit, 1, 1, &active)
            .expect_err("active asset manifest hash mismatch must be rejected");
        assert_eq!(
            error,
            "replay identity mismatch: asset manifest identity differs"
        );
        fs::remove_dir_all(directory).ok();
    }

    fn compact_v1_test_document(frame_count: u64) -> ReplayDocument {
        ReplayDocument {
            schema_version: REPLAY_SCHEMA_VERSION,
            identity: ReplayIdentity {
                stasis_version: "test".to_string(),
                release_id: "test".to_string(),
                target: "test".to_string(),
                source_sha256: "0".repeat(64),
                state_layout_sha256: "0".repeat(64),
                host_i32_count: 1,
                host_f32_count: 1,
            },
            initial_state: InitialState {
                values: Vec::new(),
                state_sha256: "0".repeat(64),
            },
            frames: (1..=frame_count)
                .map(|tick| ReplayFrame {
                    tick,
                    i32_changes: Vec::new(),
                    f32_changes: Vec::new(),
                    state_sha256: "0".repeat(64),
                })
                .collect(),
        }
    }

    #[test]
    fn compact_held_input_run_is_smaller_than_v1_frames() {
        let baseline = CompactInputSnapshot {
            i32_values: vec![1],
            f32_bits: vec![0],
        };
        let ticks = vec![baseline.clone(); 10_000];
        let segments = build_compact_segments(&baseline, &ticks).expect("build held segment");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].tick_gap, 0);
        assert_eq!(segments[0].run_ticks, 10_000);
        assert!(segments[0].i32_changes.is_empty());
        assert!(segments[0].f32_changes.is_empty());

        let compact = compact_test_document(baseline, segments, 10_000);
        let compact_bytes = compact.canonical_bytes().expect("encode compact held run");
        let v1_bytes = serde_json::to_vec(&compact_v1_test_document(10_000))
            .expect("encode v1 held-input frames");
        assert_eq!(compact_bytes.len(), 5_068);
        assert_eq!(v1_bytes.len(), 1_299_320);
        assert!(
            compact_bytes.len() < v1_bytes.len(),
            "compact {} bytes should beat v1 {} bytes",
            compact_bytes.len(),
            v1_bytes.len()
        );
    }

    #[test]
    fn compact_changes_preserve_zero_edges_and_exact_f32_bits() {
        let baseline = CompactInputSnapshot {
            i32_values: vec![0, 0],
            f32_bits: vec![0],
        };
        let ticks = vec![
            baseline.clone(),
            CompactInputSnapshot {
                i32_values: vec![1, 1],
                f32_bits: vec![0x8000_0000],
            },
            CompactInputSnapshot {
                i32_values: vec![0, 1],
                f32_bits: vec![0],
            },
            CompactInputSnapshot {
                i32_values: vec![0, 0],
                f32_bits: vec![0x7fc0_0042],
            },
        ];
        let segments = build_compact_segments(&baseline, &ticks).expect("build edge segments");
        let document = compact_test_document(baseline, segments, ticks.len() as u64);
        for (tick, expected) in ticks.iter().enumerate() {
            let actual = compact_input_at_tick(&document, tick as u64 + 1)
                .expect("reconstruct compact input");
            assert_eq!(&actual, expected, "tick {}", tick + 1);
        }
        assert_eq!(
            compact_input_at_tick(&document, 2)
                .expect("read signed-zero pulse")
                .f32_bits[0],
            0x8000_0000
        );
        assert_eq!(
            compact_input_at_tick(&document, 4)
                .expect("read NaN payload")
                .f32_bits[0],
            0x7fc0_0042
        );
    }

    #[test]
    fn compact_tick_gaps_hold_previous_snapshot() {
        let baseline = CompactInputSnapshot {
            i32_values: vec![0],
            f32_bits: vec![0],
        };
        let document = compact_test_document(
            baseline,
            vec![
                CompactInputSegment {
                    tick_gap: 0,
                    run_ticks: 1,
                    i32_changes: vec![CompactI32Change { slot: 0, value: 1 }],
                    f32_changes: Vec::new(),
                },
                CompactInputSegment {
                    tick_gap: 2,
                    run_ticks: 1,
                    i32_changes: vec![CompactI32Change { slot: 0, value: 0 }],
                    f32_changes: Vec::new(),
                },
            ],
            4,
        );
        validate_compact_document(&document).expect("validate gap coverage");
        assert_eq!(
            compact_input_at_tick(&document, 1).unwrap().i32_values,
            vec![1]
        );
        assert_eq!(
            compact_input_at_tick(&document, 2).unwrap().i32_values,
            vec![1]
        );
        assert_eq!(
            compact_input_at_tick(&document, 3).unwrap().i32_values,
            vec![1]
        );
        assert_eq!(
            compact_input_at_tick(&document, 4).unwrap().i32_values,
            vec![0]
        );
    }

    #[test]
    fn compact_validation_rejects_malformed_segments_and_initial_state_text() {
        let baseline = CompactInputSnapshot {
            i32_values: vec![0],
            f32_bits: vec![0],
        };
        let segments = vec![CompactInputSegment {
            tick_gap: 0,
            run_ticks: 2,
            i32_changes: vec![CompactI32Change { slot: 0, value: 1 }],
            f32_changes: Vec::new(),
        }];
        let valid = compact_test_document(baseline, segments, 2);
        validate_compact_document(&valid).expect("valid compact document");
        assert!(compact_input_at_tick(&valid, 0)
            .expect_err("zero selected tick must be rejected")
            .contains("outside"));
        assert!(compact_input_at_tick(&valid, 3)
            .expect_err("selected tick after total must be rejected")
            .contains("outside"));

        let mut first_gap = valid.clone();
        first_gap.segments[0].tick_gap = 1;
        assert!(validate_compact_document(&first_gap)
            .expect_err("first gap must be rejected")
            .contains("first segment"));

        let mut zero_run = valid.clone();
        zero_run.segments[0].run_ticks = 0;
        assert!(validate_compact_document(&zero_run)
            .expect_err("zero run must be rejected")
            .contains("run_ticks"));

        let checkpoint_document = compact_test_document(
            CompactInputSnapshot {
                i32_values: vec![0],
                f32_bits: vec![0],
            },
            vec![CompactInputSegment {
                tick_gap: 0,
                run_ticks: COMPACT_CHECKPOINT_INTERVAL * 2,
                i32_changes: Vec::new(),
                f32_changes: Vec::new(),
            }],
            COMPACT_CHECKPOINT_INTERVAL * 2,
        );
        let mut missing_checkpoint = checkpoint_document.clone();
        missing_checkpoint.checkpoints.pop();
        assert!(validate_compact_document(&missing_checkpoint)
            .expect_err("missing checkpoint must be rejected")
            .contains("exactly 2 checkpoints"));
        let mut off_cadence_checkpoint = checkpoint_document;
        off_cadence_checkpoint.checkpoints[0].tick += 1;
        assert!(validate_compact_document(&off_cadence_checkpoint)
            .expect_err("off-cadence checkpoint must be rejected")
            .contains("must be at tick 256"));

        let mut uncovered = valid.clone();
        uncovered.total_ticks = 3;
        uncovered.final_state.tick = 3;
        assert!(validate_compact_document(&uncovered)
            .expect_err("uncovered tick must be rejected")
            .contains("coverage"));

        let mut out_of_range = valid.clone();
        out_of_range.segments[0].i32_changes[0].slot = 1;
        assert!(validate_compact_document(&out_of_range)
            .expect_err("out-of-range slot must be rejected")
            .contains("out-of-range"));

        let mut redundant = valid.clone();
        redundant.segments[0].i32_changes[0].value = 0;
        assert!(validate_compact_document(&redundant)
            .expect_err("redundant change must be rejected")
            .contains("redundant"));

        let mut duplicate = valid.clone();
        duplicate.segments[0]
            .i32_changes
            .push(CompactI32Change { slot: 0, value: 2 });
        assert!(validate_compact_document(&duplicate)
            .expect_err("duplicate slot must be rejected")
            .contains("sorted and unique"));

        let overflow_cursor = compact_test_document(
            CompactInputSnapshot {
                i32_values: vec![0],
                f32_bits: vec![0],
            },
            vec![
                CompactInputSegment {
                    tick_gap: 0,
                    run_ticks: 1,
                    i32_changes: vec![CompactI32Change { slot: 0, value: 1 }],
                    f32_changes: Vec::new(),
                },
                CompactInputSegment {
                    tick_gap: u64::MAX,
                    run_ticks: 1,
                    i32_changes: Vec::new(),
                    f32_changes: Vec::new(),
                },
            ],
            1,
        );
        assert!(validate_compact_document(&overflow_cursor)
            .expect_err("cursor overflow must be rejected")
            .contains("overflows"));

        let mut bad_initial_hash = valid.clone();
        bad_initial_hash.initial_state.state_sha256 = "0".repeat(63);
        assert!(validate_compact_document(&bad_initial_hash)
            .expect_err("bad initial hash must be rejected")
            .contains("initial state_sha256"));

        let mut bad_initial_bits = valid;
        bad_initial_bits.initial_state.values.push(StateEntry {
            location: StateLocation::Scalar {
                path: "score".to_string(),
            },
            value: EncodedScalar {
                type_name: "i32".to_string(),
                bits: "not-hex".to_string(),
            },
        });
        assert!(validate_compact_document(&bad_initial_bits)
            .expect_err("bad initial bits must be rejected")
            .contains("hexadecimal"));

        let mut unknown_initial_type = compact_test_document(
            CompactInputSnapshot {
                i32_values: vec![0],
                f32_bits: vec![0],
            },
            vec![CompactInputSegment {
                tick_gap: 0,
                run_ticks: 1,
                i32_changes: Vec::new(),
                f32_changes: Vec::new(),
            }],
            1,
        );
        unknown_initial_type.initial_state.values.push(StateEntry {
            location: StateLocation::Scalar {
                path: "score".to_string(),
            },
            value: EncodedScalar {
                type_name: "i128".to_string(),
                bits: "0000000000000000".to_string(),
            },
        });
        assert!(validate_compact_document(&unknown_initial_type)
            .expect_err("unknown scalar type must be rejected")
            .contains("unsupported scalar type"));

        let mut overflow_initial_bits = unknown_initial_type.clone();
        overflow_initial_bits.initial_state.values[0].value = EncodedScalar {
            type_name: "u8".to_string(),
            bits: "100".to_string(),
        };
        assert!(validate_compact_document(&overflow_initial_bits)
            .expect_err("wrong scalar width must be rejected")
            .contains("exactly 2"));

        let mut invalid_bool_bits = unknown_initial_type;
        invalid_bool_bits.initial_state.values[0].value = EncodedScalar {
            type_name: "bool".to_string(),
            bits: "02".to_string(),
        };
        assert!(validate_compact_document(&invalid_bool_bits)
            .expect_err("invalid bool bits must be rejected")
            .contains("bool value"));
    }

    #[test]
    fn compact_dispatch_preserves_v1_and_rejects_duplicate_json_keys() {
        let v1 = serde_json::to_vec(&compact_v1_test_document(1)).expect("encode v1 document");
        assert!(matches!(
            decode_versioned_replay(&v1),
            Ok(VersionedReplayDocument::LegacyV1(_))
        ));

        let baseline = CompactInputSnapshot {
            i32_values: vec![0],
            f32_bits: vec![0],
        };
        let compact = compact_test_document(
            baseline,
            vec![CompactInputSegment {
                tick_gap: 0,
                run_ticks: 1,
                i32_changes: Vec::new(),
                f32_changes: Vec::new(),
            }],
            1,
        );
        let compact_json = String::from_utf8(compact.canonical_bytes().expect("encode v2"))
            .expect("compact JSON UTF-8");
        assert!(matches!(
            decode_versioned_replay(compact_json.as_bytes()),
            Ok(VersionedReplayDocument::CompactV2(_))
        ));

        let top_duplicate = compact_json.replacen(
            "\"schema_version\":2",
            "\"schema_version\":2,\"schema_version\":2",
            1,
        );
        assert!(decode_versioned_replay(top_duplicate.as_bytes())
            .expect_err("duplicate top-level key must be rejected")
            .contains("duplicate JSON object key"));

        let nested_duplicate =
            compact_json.replacen("\"tick_gap\":0", "\"tick_gap\":0,\"tick_gap\":0", 1);
        assert!(decode_versioned_replay(nested_duplicate.as_bytes())
            .expect_err("duplicate nested key must be rejected")
            .contains("duplicate JSON object key"));
    }

    #[test]
    fn compact_replay_player_streams_v2_segments_and_zero_edges() {
        let baseline = CompactInputSnapshot {
            i32_values: vec![0],
            f32_bits: vec![0],
        };
        let compact = compact_test_document(
            baseline,
            vec![CompactInputSegment {
                tick_gap: 0,
                run_ticks: 1,
                i32_changes: Vec::new(),
                f32_changes: Vec::new(),
            }],
            1,
        );
        let path = std::env::temp_dir().join(format!(
            "stasis-compact-v2-replay-{}.json",
            std::process::id()
        ));
        fs::write(
            &path,
            compact
                .canonical_bytes()
                .expect("encode v2 replay for streaming player"),
        )
        .expect("write v2 replay");
        let mut player = ReplayPlayer::load(&path).expect("load v2 replay");
        let mut host_i32 = [9];
        let mut host_f32 = [f32::from_bits(0x7fc0_0042)];
        player
            .apply_next(1, &mut host_i32, &mut host_f32)
            .expect("apply v2 baseline");
        assert_eq!(host_i32, [0]);
        assert_eq!(host_f32[0].to_bits(), 0);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn initial_state_round_trips_nominal_enum_struct_array_fields() {
        let _global_guard = crate::jit_test_support::lock();
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "main.stasis",
            "enum AssetState { None, Pending, Loading, Loaded, Failed, Cancelled, }\n\
             struct AudioAsset { handle: i32; request: i32; state: AssetState; }\n\
             global prompt_audio_assets: AudioAsset[1];\n\
             function main(): i32 { prompt_audio_assets[0].state = AssetState.Loaded; return 0; }\n",
        );
        jit.compile()
            .expect("compile replay enum collection fixture");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

        let captured = capture_initial_state(&jit).expect("capture nominal enum collection state");
        assert!(captured.values.iter().any(|entry| {
            matches!(
                &entry.location,
                StateLocation::Collection { path, field, index }
                    if path == "prompt_audio_assets" && field == "state" && *index == 0
            ) && entry.value == encode_scalar(JitScalarValue::I32(3))
        }));
        jit.write_global_collection_scalar(
            "prompt_audio_assets",
            "state",
            0,
            JitScalarValue::I32(0),
        )
        .expect("clear enum collection field");

        restore_initial_state(&jit, &captured).expect("restore nominal enum collection state");

        assert_eq!(
            jit.read_global_collection_scalar("prompt_audio_assets", "state", 0),
            Ok(JitScalarValue::I32(3))
        );
        assert_eq!(
            simulation_state_hash(&jit),
            Ok(captured.state_sha256.clone())
        );
    }

    #[test]
    fn unfinished_tick_can_be_discarded_after_guest_shutdown() {
        let mut recorder = ReplayRecorder {
            output: PathBuf::from("unused.replay.json"),
            legacy: Some(LegacyReplayRecorder {
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
            }),
            compact: None,
        };
        recorder
            .begin_tick(1, &[7, 0], &[1.0])
            .expect("begin unfinished tick");
        recorder.discard_tick(1).expect("discard unfinished tick");
        assert!(recorder
            .legacy
            .as_ref()
            .expect("legacy recorder")
            .document
            .frames
            .is_empty());
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
    fn compact_contract_rejects_reachable_unknown_time_but_ignores_unreachable_helper() {
        let _global_guard = crate::jit_test_support::lock();
        let compile = |tick: &str| {
            let mut jit = JitProcess::new();
            jit.upsert_file(
                "main.stasis",
                format!(
                    "extern function time(): i32; \
                     function unused_clock(): i32 {{ return time(); }} \
                     function main(): i32 {{ return 0; }} \
                     function tick(): i32 {{ {tick} }} \
                     function render(): i32 {{ return 0; }}"
                ),
            );
            jit.compile().expect("compile compact effect fixture");
            jit
        };

        let safe = compile("return 0;");
        validate_compact_replay_contract(&safe)
            .expect("unreachable host observation must not reject replay");

        let unsafe_jit = compile("return time();");
        let error = validate_compact_replay_contract(&unsafe_jit)
            .expect_err("reachable wall clock must reject compact replay");
        assert!(error.contains("time (unknown)"), "{error}");
    }

    #[test]
    fn compact_contract_defaults_to_rejecting_async_and_host_returning_effects() {
        let import = |name: &str,
                      symbol: &str,
                      parameter_count: usize,
                      returns_void: bool|
         -> ProgramExternImport {
            ProgramExternImport {
                name: name.to_string(),
                symbol: symbol.to_string(),
                params: vec![0; parameter_count],
                return_type: 0,
                returns_void,
            }
        };
        assert!(!compact_import_is_safe(
            &import("fixture", "stasis_jit_asset_request_sprite", 3, false),
            "graphics"
        ));
        assert!(!compact_import_is_safe(
            &import("fixture", "stasis_jit_asset_request_audio", 1, false),
            "audio"
        ));
        assert!(!compact_import_is_safe(
            &import("fixture", "stasis_jit_audio_init", 3, false),
            "audio"
        ));
        assert!(compact_import_is_safe(
            &import("fixture", "stasis_jit_measure_text", 2, false),
            "graphics"
        ));
        assert!(compact_import_is_safe(
            &import("fixture", "stasis_jit_gfx_release_sprite", 1, true),
            "graphics"
        ));
        assert!(compact_import_is_safe(
            &import("fixture", "stasis_jit_audio_stop", 1, true),
            "audio"
        ));
        assert!(compact_import_is_safe(
            &import("sys_memcpy_i32", "stasis_jit_sys_memcpy_i32", 5, true),
            "memory"
        ));
        assert!(!compact_import_is_safe(
            &import("copy", "stasis_jit_sys_memcpy_i32", 5, true),
            "memory"
        ));
        assert!(!compact_import_is_safe(
            &import("fixture", "environment_value", 0, false),
            "memory"
        ));
        assert!(!compact_import_is_safe(
            &import("fixture", "stasis_jit_audio_stop", 1, false),
            "audio"
        ));

        let _global_guard = crate::jit_test_support::lock();
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "main.stasis",
            "global source: i32[1]; global destination: i32[1]; \
             extern function @internal @effects(memory) sys_memcpy_i32(dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32): void; \
             function main(): i32 { return 0; } \
             function tick(): i32 { sys_memcpy_i32(destination, 0, source, 0, 1); return destination[0]; } \
             function render(): i32 { return 0; }",
        );
        jit.compile().expect("compile exact memory-import fixture");
        validate_compact_replay_contract(&jit)
            .expect("exact deterministic bulk-memory import must remain replay compatible");
    }

    #[test]
    fn compact_recorder_rejects_tick_past_limit_before_pending_work() {
        let _global_guard = crate::jit_test_support::lock();
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "main.stasis",
            "function main(): i32 { return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        jit.compile().expect("compile compact tick-limit fixture");
        let output = std::env::temp_dir().join(format!(
            "stasis-compact-tick-limit-{}.json",
            std::process::id()
        ));
        let mut recorder =
            ReplayRecorder::start_compact_with_defaults(output, &jit, 1, 1, &[], &[])
                .expect("start compact tick-limit recorder");
        recorder
            .compact
            .as_mut()
            .expect("compact recorder")
            .document
            .total_ticks = MAX_REPLAY_FRAMES as u64;
        let error = recorder
            .begin_tick(MAX_REPLAY_FRAMES as u64 + 1, &[0], &[0.0])
            .expect_err("tick beyond compact limit must fail before pending work");
        assert!(error.contains("tick limit"), "{error}");
        assert!(recorder
            .compact
            .as_ref()
            .expect("compact recorder")
            .pending
            .is_none());
    }

    #[test]
    fn compact_recording_omits_unused_mouse_activity_and_rejects_usage_mismatch() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "stasis-compact-record-replay-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temp directory");
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "main.stasis",
            "global host_i32: i32[768]; global host_f32: f32[64]; global score: i32; \
             function main(): i32 { score = 0; return 0; } \
             function tick(): i32 { let key: i32 = host_i32[32]; score += 1; return key - key; } \
             function render(): i32 { return 0; }",
        );
        jit.compile().expect("compile compact replay fixture");
        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));
        let (observed_i32, observed_f32) =
            compact_observed_fields(&jit).expect("derive observed fields");
        assert_eq!(
            observed_i32
                .iter()
                .map(|field| field.index)
                .collect::<Vec<_>>(),
            [32]
        );
        assert!(observed_f32.is_empty());
        assert_eq!(
            compact_input_usage_hash(&observed_i32, &observed_f32),
            jit.program_snapshot()
                .expect("compiled program snapshot")
                .host_frame_input_usage()
                .identity_sha256(),
            "desktop replay and packaged metadata must share one input identity"
        );

        let record = |path: &Path, jit: &JitProcess, mouse_bits: [u32; 3]| {
            assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));
            let mut recorder = ReplayRecorder::start_compact_with_defaults(
                path.to_path_buf(),
                jit,
                768,
                64,
                &observed_i32,
                &observed_f32,
            )
            .expect("start compact recorder");
            for (offset, key) in [0, 1, 1].into_iter().enumerate() {
                let tick = offset as u64 + 1;
                let mut host_i32 = vec![0; 768];
                let mut host_f32 = vec![0.0; 64];
                host_i32[32] = key;
                host_f32[0] = f32::from_bits(mouse_bits[offset]);
                recorder
                    .begin_tick(tick, &host_i32, &host_f32)
                    .expect("begin compact tick");
                assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
                recorder.finish_tick(jit).expect("finish compact tick");
            }
            recorder.publish().expect("publish compact replay");
            fs::read(path).expect("read compact replay")
        };

        let first_path = directory.join("first.replay.json");
        let second_path = directory.join("second.replay.json");
        let first = record(&first_path, &jit, [0, 1.0_f32.to_bits(), 2.0_f32.to_bits()]);
        let second = record(
            &second_path,
            &jit,
            [0x7fc0_0042, (-0.0_f32).to_bits(), 3.0_f32.to_bits()],
        );
        assert_eq!(first, second, "unused mouse activity must not affect bytes");

        let mut tampered: CompactReplayDocument =
            serde_json::from_slice(&first).expect("parse compact replay");
        tampered.identity.observed_i32[0].path = "keys[999]".to_string();
        tampered.identity.input_usage_sha256 = compact_input_usage_hash(
            &tampered.identity.observed_i32,
            &tampered.identity.observed_f32,
        );
        let tampered_path = directory.join("tampered.replay.json");
        fs::write(
            &tampered_path,
            tampered.canonical_bytes().expect("encode tampered replay"),
        )
        .expect("write tampered replay");
        let player = ReplayPlayer::load(&tampered_path).expect("load structurally valid replay");
        assert!(player
            .initialize(&jit)
            .expect_err("active input usage must reject tampered descriptors")
            .contains("compiler-observed input usage differs"));

        assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));
        let mut player = ReplayPlayer::load(&first_path).expect("load compact replay");
        player.initialize(&jit).expect("initialize compact replay");
        let mut replay_i32 = vec![0; 768];
        let mut replay_f32 = vec![0.0; 64];
        for tick in 1..=3 {
            player
                .apply_next(tick, &mut replay_i32, &mut replay_f32)
                .expect("apply compact input");
            jit.write_global_collection_scalar(
                "host_i32",
                "",
                32,
                JitScalarValue::I32(replay_i32[32]),
            )
            .expect("apply key slot to guest HostFrame");
            assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
            if tick == 3 {
                jit.write_global_scalar("score", JitScalarValue::I32(99))
                    .expect("corrupt final state");
                let diagnostic = player
                    .verify_tick(tick, &jit)
                    .expect_err("final corruption must diverge");
                assert!(diagnostic.contains("within ticks 1..=3"), "{diagnostic}");
                assert!(diagnostic.contains("checkpoint 3"), "{diagnostic}");
            } else {
                player.verify_tick(tick, &jit).expect("pre-final tick");
            }
        }
        fs::remove_dir_all(directory).ok();
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
        assert_eq!(
            recorder
                .legacy
                .as_ref()
                .expect("legacy recorder")
                .document
                .initial_state
                .values
                .len(),
            1
        );
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
