use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stasis_assets::{prepare_asset_bundle, sha256_bytes, DEFAULT_ASSET_MANIFEST_PATH};
use stasis_compiler::backend::aot::{AotEngineBundle, AotProcess};
use stasis_compiler::backend::jit::{JitEnginePackage, JitProcess};
use stasis_compiler::backend::program_snapshot::{
    ProgramReplayStateEntry, ProgramReplayStateSnapshot, ProgramSnapshot,
};
use stasis_compiler::backend::state_layout::{
    aot_storage_symbol, AotStorageSymbolKind, StateLayout,
};
use stasis_compiler::backend::{AotOptimizationProfile, EngineEntrypoints, ReachabilityPolicy};
use stasis_jit::{
    link_objects_to_dynamic_library, link_objects_to_executable, AotCompileConfig, AotLinkConfig,
};
use stasis_runner::swap::contracts::{
    AotFunctionSymbol, CompileRequest, CompileResult, Diagnostic, DiagnosticSeverity, FnId,
    FunctionPatch, FunctionPatchSet, JitCodePtrOverride, LayoutHash, RequestId, TargetMode,
};
use stasis_runner::swap::pipeline::CompilerBackend;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

pub(crate) struct PreparedJitSwap {
    pub(crate) request_id: stasis_runner::swap::contracts::RequestId,
    pub(crate) candidate: JitProcess,
}

pub struct IncrementalCompilerBackend {
    project_root: Option<PathBuf>,
    source_by_path: BTreeMap<String, String>,
    jit_process: JitProcess,
    jit_process_seeded: bool,
    aot_compile_config: AotCompileConfig,
    aot_link_config: AotLinkConfig,
    aot_artifact_root: std::path::PathBuf,
    enable_aot_link_step: bool,
    last_jit_engine_package: Option<JitEnginePackage>,
    last_aot_engine_bundle: Option<AotEngineBundle>,
    prepared_jit_swap_tx: Option<SyncSender<PreparedJitSwap>>,
    pending_jit_candidate: Option<JitProcess>,
    last_program_snapshot: Option<ProgramSnapshot>,
    last_jit_source_diagnostic: Option<stasis_compiler::SourceDiagnostic>,
    last_aot_source_diagnostic: Option<stasis_compiler::SourceDiagnostic>,
}

fn stable_absolute_path(path: &Path) -> PathBuf {
    let absolute = path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    });
    #[cfg(windows)]
    {
        let text = absolute.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    absolute
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfHostedAotCliSummary {
    pub source_file_count: usize,
    pub linked_image_path: PathBuf,
    pub entry_symbol: String,
    pub ir_bundle_path: PathBuf,
    pub object_bundle_path: PathBuf,
    pub object_file_names: Vec<String>,
    #[serde(skip)]
    pub program_snapshot: Option<ProgramSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AotPatchManifest {
    request_id: u64,
    artifact_paths: Vec<String>,
    linked_image_path: Option<String>,
    linked_image_size_bytes: Option<u64>,
    linked_image_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SelfHostObjectBundle {
    entry_symbol: String,
    object_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EngineFunctionEntry {
    path: String,
    name: String,
    parameter_count: usize,
    symbol_id: String,
    fn_id: FnId,
}

fn is_zero_argument_tick(entry: &EngineFunctionEntry) -> bool {
    entry.name == "tick" && entry.parameter_count == 0
}

fn is_host_lifecycle_entry(entry: &EngineFunctionEntry) -> bool {
    matches!(entry.name.as_str(), "main" | "render" | "on_code_swap")
        || is_zero_argument_tick(entry)
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestFunctionRow {
    function_id: u32,
    symbol_id: String,
    name: String,
    symbol: String,
    #[serde(default)]
    parameter_count: usize,
    return_type: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestStringLiteralRow {
    id: i32,
    value: String,
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestCollectionMaxLengthRow {
    path: String,
    max_length: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestHotRenderImageRow {
    #[serde(default)]
    identity: String,
    logical_path: String,
    logical_width: u32,
    logical_height: u32,
    max_renders_per_render: Option<u64>,
    atlas_eligible: bool,
    grouping_key: String,
    #[serde(default)]
    estimated_distinct_transitions: u64,
    #[serde(default)]
    group_member_count: u32,
    #[serde(default)]
    group_logical_pixel_area: u64,
    #[serde(default)]
    group_max_logical_width: u32,
    #[serde(default)]
    group_max_logical_height: u32,
    #[serde(default)]
    backend_constraints: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestHotRenderTransitionRow {
    from_identity: String,
    to_identity: String,
    max_transitions_per_render: Option<u64>,
    validity: String,
    #[serde(default)]
    unknown_cause: Option<String>,
    #[serde(default)]
    provenance: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestHotRenderTransitionAnalysisRow {
    validity: String,
    #[serde(default)]
    unknown_causes: Vec<String>,
    pair_count: Option<u32>,
    published_pair_count: u32,
    omitted_pair_count: Option<u32>,
    pair_limit: u32,
    max_pair_weight: u64,
}
fn is_zero_argument_manifest_function(row: &EngineBundleManifestFunctionRow, name: &str) -> bool {
    row.name == name && row.parameter_count == 0
}

fn packaged_render_alias(
    manifest: &EngineBundleManifest,
    render: &EngineBundleManifestFunctionRow,
) -> Result<PackagedRenderAlias, String> {
    if !matches!(render.return_type, 0 | 1) {
        return Err(format!(
            "engine bundle render callback must return void or i32, found type id {}",
            render.return_type
        ));
    }
    let reset_count = manifest
        .functions
        .iter()
        .filter(|row| row.name == "gfx_cmd_construction_reset")
        .count();
    let finish_count = manifest
        .functions
        .iter()
        .filter(|row| row.name == "gfx_cmd_construction_finish")
        .count();
    if reset_count != finish_count {
        return Err(
            "engine bundle render construction lifecycle requires both reset and finish helpers"
                .to_string(),
        );
    }
    if reset_count > 1 {
        return Err(
            "engine bundle render construction lifecycle requires exactly one reset and finish helper"
                .to_string(),
        );
    }
    let (reset_symbol, finish_symbol) = match manifest.render_construction_lifecycle_version {
        0 => {
            if reset_count != 0 {
                return Err(
                    "engine bundle render lifecycle version 0 must omit construction helpers"
                        .to_string(),
                );
            }
            (None, None)
        }
        1 => {
            let reset = manifest
                .functions
                .iter()
                .find(|row| row.name == "gfx_cmd_construction_reset")
                .ok_or_else(|| {
                    "engine bundle render lifecycle is missing gfx_cmd_construction_reset"
                        .to_string()
                })?;
            if reset.parameter_count != 0 || reset.return_type != 0 {
                return Err(
                    "engine bundle gfx_cmd_construction_reset must have signature void()"
                        .to_string(),
                );
            }
            let finish = manifest
                .functions
                .iter()
                .find(|row| row.name == "gfx_cmd_construction_finish")
                .ok_or_else(|| {
                    "engine bundle render lifecycle is missing gfx_cmd_construction_finish"
                        .to_string()
                })?;
            if finish.parameter_count != 1 || finish.return_type != 1 {
                return Err(
                    "engine bundle gfx_cmd_construction_finish must have signature i32(i32)"
                        .to_string(),
                );
            }
            (Some(reset.symbol.clone()), Some(finish.symbol.clone()))
        }
        version => {
            return Err(format!(
                "unsupported engine bundle render construction lifecycle version {version}"
            ))
        }
    };
    Ok(PackagedRenderAlias {
        target_symbol: render.symbol.clone(),
        returns_i32: render.return_type == 1,
        reset_symbol,
        finish_symbol,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifest {
    #[serde(default)]
    optimization_profile: Option<String>,
    #[serde(default)]
    render_construction_lifecycle_version: u32,
    functions: Vec<EngineBundleManifestFunctionRow>,
    #[serde(default)]
    string_literals: Option<Vec<EngineBundleManifestStringLiteralRow>>,
    #[serde(default)]
    collection_max_lengths: Option<Vec<EngineBundleManifestCollectionMaxLengthRow>>,
    #[serde(default)]
    hot_render_metadata_version: Option<u32>,
    #[serde(default)]
    hot_render_images: Option<Vec<EngineBundleManifestHotRenderImageRow>>,
    #[serde(default)]
    hot_render_transitions: Option<Vec<EngineBundleManifestHotRenderTransitionRow>>,
    #[serde(default)]
    hot_render_transition_analysis: Option<EngineBundleManifestHotRenderTransitionAnalysisRow>,
}

fn read_engine_bundle_manifest(path: &Path) -> Result<EngineBundleManifest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read AOT engine bundle manifest {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "failed to parse AOT engine bundle manifest {}: {error}",
            path.display()
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HotRenderRuntimePolicy {
    version: u32,
    images: Vec<stasis_dynload::HotRenderRuntimeImage>,
    transitions: Vec<stasis_dynload::HotRenderRuntimeTransition>,
    analysis: stasis_dynload::HotRenderRuntimeTransitionAnalysis,
}

impl HotRenderRuntimePolicy {
    #[cfg(test)]
    pub(crate) fn for_test(images: Vec<stasis_dynload::HotRenderRuntimeImage>) -> Self {
        Self {
            version: stasis_dynload::HOT_RENDER_METADATA_VERSION,
            images,
            transitions: Vec::new(),
            analysis: stasis_dynload::HotRenderRuntimeTransitionAnalysis::default(),
        }
    }

    pub(crate) fn publish(&self) {
        stasis_dynload::replace_hot_render_metadata_v4(
            self.version,
            &self.images,
            &self.transitions,
            &self.analysis,
        );
    }
}

pub(crate) fn snapshot_hot_render_policy(snapshot: &ProgramSnapshot) -> HotRenderRuntimePolicy {
    let images = snapshot
        .hot_render_images()
        .iter()
        .map(|image| stasis_dynload::HotRenderRuntimeImage {
            identity: image.identity.clone(),
            logical_path: image.logical_path.clone(),
            logical_width: image.logical_width,
            logical_height: image.logical_height,
            max_renders_per_render: image.max_renders_per_render,
            atlas_eligible: image.atlas_eligible,
            grouping_key: image.grouping_key.clone(),
            estimated_distinct_transitions: image.estimated_distinct_transitions,
            group_member_count: image.group_member_count,
            group_logical_pixel_area: image.group_logical_pixel_area,
            group_max_logical_width: image.group_max_logical_width,
            group_max_logical_height: image.group_max_logical_height,
            backend_constraints: image.backend_constraints.clone(),
        })
        .collect::<Vec<_>>();
    let transitions = snapshot
        .hot_render_transitions()
        .iter()
        .map(|edge| stasis_dynload::HotRenderRuntimeTransition {
            from_identity: edge.from_identity.clone(),
            to_identity: edge.to_identity.clone(),
            max_transitions_per_render: edge.max_transitions_per_render,
            validity: match edge.validity {
                stasis_compiler::backend::hot_render::HotRenderTransitionValidity::Finite =>
                    "finite".to_string(),
                stasis_compiler::backend::hot_render::HotRenderTransitionValidity::Unknown =>
                    "unknown".to_string(),
            },
            unknown_cause: edge.unknown_cause.clone(),
            provenance: edge.provenance.iter().map(|source| match source {
                stasis_compiler::backend::hot_render::HotRenderTransitionProvenance::SequenceJoin => "sequence_join",
                stasis_compiler::backend::hot_render::HotRenderTransitionProvenance::BranchMaximum => "branch_maximum",
                stasis_compiler::backend::hot_render::HotRenderTransitionProvenance::FixedRepeatInternal => "fixed_repeat_internal",
                stasis_compiler::backend::hot_render::HotRenderTransitionProvenance::FixedRepeatBoundary => "fixed_repeat_boundary",
            }.to_string()).collect(),
        })
        .collect();
    let summary = snapshot.hot_render_transition_analysis();
    let analysis = stasis_dynload::HotRenderRuntimeTransitionAnalysis {
        validity: match summary.validity {
            stasis_compiler::backend::hot_render::HotRenderTransitionAnalysisValidity::Complete => "complete".to_string(),
            stasis_compiler::backend::hot_render::HotRenderTransitionAnalysisValidity::Incomplete => "incomplete".to_string(),
        },
        unknown_causes: summary.unknown_causes.clone(),
        pair_count: summary.pair_count,
        published_pair_count: summary.published_pair_count,
        omitted_pair_count: summary.omitted_pair_count,
        pair_limit: summary.pair_limit,
        max_pair_weight: summary.max_pair_weight,
    };
    HotRenderRuntimePolicy {
        version: stasis_compiler::backend::hot_render::HOT_RENDER_METADATA_VERSION,
        images,
        transitions,
        analysis,
    }
}

fn manifest_hot_render_policy(manifest: &EngineBundleManifest) -> HotRenderRuntimePolicy {
    let images = manifest
        .hot_render_images
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|image| stasis_dynload::HotRenderRuntimeImage {
            identity: image.identity.clone(),
            logical_path: image.logical_path.clone(),
            logical_width: image.logical_width,
            logical_height: image.logical_height,
            max_renders_per_render: image.max_renders_per_render,
            atlas_eligible: image.atlas_eligible && image.backend_constraints.is_some(),
            grouping_key: image.grouping_key.clone(),
            estimated_distinct_transitions: image.estimated_distinct_transitions,
            group_member_count: image.group_member_count,
            group_logical_pixel_area: image.group_logical_pixel_area,
            group_max_logical_width: image.group_max_logical_width,
            group_max_logical_height: image.group_max_logical_height,
            backend_constraints: image.backend_constraints.clone().unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    let transitions = manifest
        .hot_render_transitions
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|edge| stasis_dynload::HotRenderRuntimeTransition {
            from_identity: edge.from_identity.clone(),
            to_identity: edge.to_identity.clone(),
            max_transitions_per_render: edge.max_transitions_per_render,
            validity: edge.validity.clone(),
            unknown_cause: edge.unknown_cause.clone(),
            provenance: edge.provenance.clone(),
        })
        .collect();
    let analysis = manifest
        .hot_render_transition_analysis
        .as_ref()
        .map_or_else(
            stasis_dynload::HotRenderRuntimeTransitionAnalysis::default,
            |summary| stasis_dynload::HotRenderRuntimeTransitionAnalysis {
                validity: summary.validity.clone(),
                unknown_causes: summary.unknown_causes.clone(),
                pair_count: summary.pair_count,
                published_pair_count: summary.published_pair_count,
                omitted_pair_count: summary.omitted_pair_count,
                pair_limit: summary.pair_limit,
                max_pair_weight: summary.max_pair_weight,
            },
        );
    HotRenderRuntimePolicy {
        version: manifest.hot_render_metadata_version.unwrap_or_default(),
        images,
        transitions,
        analysis,
    }
}
pub(crate) fn load_manifest_hot_render_policy(
    path: &Path,
) -> Result<HotRenderRuntimePolicy, String> {
    read_engine_bundle_manifest(path).map(|manifest| manifest_hot_render_policy(&manifest))
}

#[derive(Debug, Clone, Deserialize)]
struct StructMetaFieldExportRow {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "jsonPath")]
    json_path: String,
    #[serde(default, rename = "csvColumn")]
    csv_column: Option<String>,
    size: usize,
    #[serde(rename = "type")]
    field_type: String,
    #[serde(rename = "arrayCount")]
    array_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct StructMetaExportFile {
    #[serde(default, rename = "globalName")]
    global_name: String,
    #[serde(default, rename = "csvTable")]
    csv_table: Option<crate::CsvTableMetadata>,
    fields: Vec<StructMetaFieldExportRow>,
}

#[derive(Debug, Clone, Default)]
struct PackagedAotSupportFiles {
    data_bind_json_rel: Option<String>,
    data_bind_meta_rel: Option<String>,
    runtime_fields: Vec<PackagedRuntimeField>,
}

#[derive(Debug, Clone)]
struct PackagedFunctionAlias {
    alias: &'static str,
    target_symbol: String,
    returns_i32: bool,
}

#[derive(Debug, Clone)]
struct PackagedRenderAlias {
    target_symbol: String,
    returns_i32: bool,
    reset_symbol: Option<String>,
    finish_symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct PackagedRuntimeField {
    name: String,
    size: usize,
    field_type: String,
    array_count: usize,
    initial_value: Option<serde_json::Value>,
    collection_path: Option<String>,
    collection_field: Option<String>,
    /// Original state path used for runtime hashes; `name` is a C symbol.
    runtime_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SourceCacheDelta {
    touched_paths: Vec<String>,
    removed_paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct DirectAotArtifactBundle {
    output_dir: PathBuf,
    object_paths_by_function: BTreeMap<u32, (String, PathBuf)>,
    linked_image_path: Option<PathBuf>,
    linked_image_size_bytes: Option<u64>,
    linked_image_sha256: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SelfHostedAotCliOptions {
    summary_file_path: Option<PathBuf>,
    entry_file: Option<PathBuf>,
    desktop_network: Option<DesktopNetworkLink>,
    artifact_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopNetworkLink {
    library: PathBuf,
    include_dir: PathBuf,
    mode: DesktopNetworkMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopNetworkMode {
    Host,
    Client,
}

impl DesktopNetworkMode {
    fn cmake_value(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Client => "client",
        }
    }
}

impl SelfHostedAotCliOptions {
    pub fn new(summary_file_path: Option<PathBuf>, entry_file: Option<PathBuf>) -> Self {
        Self {
            summary_file_path,
            entry_file,
            desktop_network: None,
            artifact_root: None,
        }
    }

    fn with_artifact_root(mut self, artifact_root: PathBuf) -> Self {
        self.artifact_root = Some(artifact_root);
        self
    }

    fn with_desktop_network(
        mut self,
        library: PathBuf,
        include_dir: PathBuf,
        mode: DesktopNetworkMode,
    ) -> Self {
        self.desktop_network = Some(DesktopNetworkLink {
            library,
            include_dir,
            mode,
        });
        self
    }
}

impl IncrementalCompilerBackend {
    fn compile_jit_candidate_from_cache(
        &mut self,
        source_delta: &SourceCacheDelta,
    ) -> Result<JitProcess, String> {
        self.sync_jit_process_sources(source_delta);
        let mut candidate = if self.prepared_jit_swap_tx.is_some() {
            self.jit_process.staged_candidate()
        } else {
            std::mem::take(&mut self.jit_process)
        };
        if let Err(error) = candidate.compile_staged() {
            self.last_jit_source_diagnostic = candidate.last_source_diagnostic().cloned();
            if self.prepared_jit_swap_tx.is_none() {
                self.jit_process = candidate;
            }
            return Err(format!("rust-native JIT compile failed: {error:?}"));
        }
        candidate.validate_on_code_swap_signature()?;
        self.last_jit_source_diagnostic = None;
        Ok(candidate)
    }

    pub fn new() -> Self {
        Self::new_inner(None)
    }

    pub fn new_for_project(project_root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = stable_absolute_path(&project_root.into());
        if !root.is_absolute() {
            return Err(format!(
                "compiler project root must be absolute: {}",
                root.display()
            ));
        }
        Ok(Self::new_inner(Some(root)))
    }

    fn new_inner(project_root: Option<PathBuf>) -> Self {
        let cache_root = std::env::var_os("STASIS_CACHE_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".stasis_cache")
            });
        Self {
            project_root,
            source_by_path: BTreeMap::new(),
            jit_process: JitProcess::new(),
            jit_process_seeded: false,
            aot_compile_config: AotCompileConfig::default(),
            aot_link_config: AotLinkConfig::default(),
            aot_artifact_root: cache_root.join("aot"),
            enable_aot_link_step: std::env::var("STASIS_AOT_LINK_ARTIFACTS")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            last_jit_engine_package: None,
            last_aot_engine_bundle: None,
            prepared_jit_swap_tx: None,
            pending_jit_candidate: None,
            last_program_snapshot: None,
            last_jit_source_diagnostic: None,
            last_aot_source_diagnostic: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_prepared_jit_swaps(
        prepared_jit_swap_tx: SyncSender<PreparedJitSwap>,
    ) -> Self {
        let mut backend = Self::new();
        backend.prepared_jit_swap_tx = Some(prepared_jit_swap_tx);
        backend
    }

    pub(crate) fn new_for_project_with_prepared_jit_swaps(
        project_root: impl Into<PathBuf>,
        prepared_jit_swap_tx: SyncSender<PreparedJitSwap>,
    ) -> Result<Self, String> {
        let mut backend = Self::new_for_project(project_root)?;
        backend.prepared_jit_swap_tx = Some(prepared_jit_swap_tx);
        Ok(backend)
    }

    pub fn new_self_host_aot_cli(aot_artifact_root: PathBuf) -> Self {
        let mut backend = Self::new();
        backend.aot_artifact_root = aot_artifact_root;
        if cfg!(windows) && backend.aot_link_config.linker_path.is_none() {
            backend.aot_link_config.linker_path = resolve_installed_lld_link()
                .or_else(|| ensure_rust_lld_link_wrapper(&backend.aot_artifact_root));
        }
        backend.enable_aot_link_step = false;
        backend
    }

    #[cfg(test)]
    fn with_aot_config(
        aot_compile_config: AotCompileConfig,
        aot_artifact_root: std::path::PathBuf,
    ) -> Self {
        Self {
            project_root: None,
            source_by_path: BTreeMap::new(),
            jit_process: JitProcess::new(),
            jit_process_seeded: false,
            aot_compile_config,
            aot_link_config: AotLinkConfig::default(),
            aot_artifact_root,
            enable_aot_link_step: false,
            last_jit_engine_package: None,
            last_aot_engine_bundle: None,
            prepared_jit_swap_tx: None,
            pending_jit_candidate: None,
            last_program_snapshot: None,
            last_jit_source_diagnostic: None,
            last_aot_source_diagnostic: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_aot_compile_and_link_config(
        aot_compile_config: AotCompileConfig,
        aot_link_config: AotLinkConfig,
        aot_artifact_root: std::path::PathBuf,
        enable_aot_link_step: bool,
    ) -> Self {
        Self {
            project_root: None,
            source_by_path: BTreeMap::new(),
            jit_process: JitProcess::new(),
            jit_process_seeded: false,
            aot_compile_config,
            aot_link_config,
            aot_artifact_root,
            enable_aot_link_step,
            last_jit_engine_package: None,
            last_aot_engine_bundle: None,
            prepared_jit_swap_tx: None,
            pending_jit_candidate: None,
            last_program_snapshot: None,
            last_jit_source_diagnostic: None,
            last_aot_source_diagnostic: None,
        }
    }
}

fn snapshot_function_entries(snapshot: &ProgramSnapshot) -> Vec<EngineFunctionEntry> {
    snapshot
        .functions()
        .iter()
        .filter_map(|function| {
            snapshot
                .files()
                .get(function.file_id as usize)
                .map(|file| EngineFunctionEntry {
                    path: file.path.clone(),
                    name: function.name.clone(),
                    parameter_count: function.params.len(),
                    symbol_id: function.symbol_id.to_string(),
                    fn_id: FnId(function.id),
                })
        })
        .collect()
}

impl Default for IncrementalCompilerBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CompilerBackend for IncrementalCompilerBackend {
    fn compile(&mut self, request: CompileRequest) -> CompileResult {
        let request_id = request.request_id;
        let accepted_jit = self.jit_process.staged_candidate();
        let accepted_snapshot = self.last_program_snapshot.clone();
        let accepted_jit_package = self.last_jit_engine_package.clone();
        let accepted_aot_bundle = self.last_aot_engine_bundle.clone();
        let accepted_pending = self
            .pending_jit_candidate
            .as_ref()
            .map(JitProcess::staged_candidate);
        let mut result = self.compile_request(request);
        if result.status == stasis_runner::swap::contracts::CompileStatus::Success {
            if let Err(message) = self.publish_prepared_jit_candidate(request_id) {
                result = CompileResult::failed(
                    request_id,
                    vec![Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: None,
                        message,
                        path: None,
                        line: None,
                        column: None,
                    }],
                );
            }
        }
        if result.status != stasis_runner::swap::contracts::CompileStatus::Success {
            self.jit_process = accepted_jit;
            self.last_program_snapshot = accepted_snapshot;
            self.last_jit_engine_package = accepted_jit_package;
            self.last_aot_engine_bundle = accepted_aot_bundle;
            self.pending_jit_candidate = accepted_pending;
        }
        result
    }
}

impl IncrementalCompilerBackend {
    fn ensure_project_root(&mut self, changed_files: &[PathBuf]) -> Result<String, String> {
        if self.project_root.is_none() {
            let first = changed_files.first().ok_or_else(|| {
                "compiler request has no project root or changed file".to_string()
            })?;
            let absolute = stable_absolute_path(first);
            let entry_parent = absolute
                .parent()
                .ok_or_else(|| format!("compiler entry has no parent: {}", first.display()))?;
            let root = entry_parent
                .ancestors()
                .find(|ancestor| {
                    ancestor.join("Cargo.toml").is_file() && ancestor.join("docs/spec.md").is_file()
                })
                .unwrap_or(entry_parent);
            self.project_root = Some(root.to_path_buf());
        }
        Ok(self
            .project_root
            .as_ref()
            .expect("project root initialized")
            .to_string_lossy()
            .to_string())
    }

    fn compile_request(&mut self, request: CompileRequest) -> CompileResult {
        if let Err(message) = self.ensure_project_root(&request.changed_files) {
            return CompileResult::failed(
                request.request_id,
                vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: None,
                    message,
                    path: request.changed_files.first().cloned(),
                    line: None,
                    column: None,
                }],
            );
        }
        let source_delta = match self.refresh_cached_sources(&request.changed_files) {
            Ok(delta) => delta,
            Err(message) => {
                return CompileResult::failed(
                    request.request_id,
                    vec![Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: None,
                        message,
                        path: request.changed_files.first().cloned(),
                        line: None,
                        column: None,
                    }],
                );
            }
        };

        if request.target_mode == TargetMode::JitDev {
            let candidate = match self.compile_jit_candidate_from_cache(&source_delta) {
                Ok(candidate) => candidate,
                Err(message) => {
                    return CompileResult::failed(
                        request.request_id,
                        vec![self.runner_diagnostic_from_source(
                            self.last_jit_source_diagnostic.as_ref(),
                            message,
                            request.changed_files.first().cloned(),
                        )],
                    )
                }
            };
            let entries = snapshot_function_entries(
                candidate
                    .program_snapshot()
                    .expect("compiled JIT candidate snapshot"),
            );
            let engine = entries.iter().any(is_zero_argument_tick)
                && entries.iter().any(|entry| entry.name == "render");
            if engine {
                return self.compile_engine_mode_contract_request(
                    &request,
                    entries.iter().any(|entry| entry.name == "on_code_swap"),
                    &source_delta,
                    &entries,
                    Some(candidate),
                    None,
                );
            }
            return match self.compile_jit_non_engine_contract_request(
                &request,
                &source_delta,
                &entries,
                Some(candidate),
            ) {
                Ok(result) => result,
                Err(message) => CompileResult::failed(
                    request.request_id,
                    vec![Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: None,
                        message,
                        path: request.changed_files.first().cloned(),
                        line: None,
                        column: None,
                    }],
                ),
            };
        }
        let candidate =
            match self.compile_aot_process_from_source_cache(ReachabilityPolicy::Development) {
                Ok(candidate) => candidate,
                Err(message) => {
                    return CompileResult::failed(
                        request.request_id,
                        vec![self.runner_diagnostic_from_source(
                            self.last_aot_source_diagnostic.as_ref(),
                            message,
                            request.changed_files.first().cloned(),
                        )],
                    )
                }
            };
        let function_entries = snapshot_function_entries(
            candidate
                .program_snapshot()
                .expect("compiled AOT candidate snapshot"),
        );
        let has_tick_entrypoint = function_entries.iter().any(is_zero_argument_tick);
        let has_render_entrypoint = function_entries.iter().any(|entry| entry.name == "render");
        let has_on_code_swap_entrypoint = function_entries
            .iter()
            .any(|entry| entry.name == "on_code_swap");
        let use_engine_mode_contracts = has_tick_entrypoint && has_render_entrypoint;
        if use_engine_mode_contracts {
            return self.compile_engine_mode_contract_request(
                &request,
                has_on_code_swap_entrypoint,
                &source_delta,
                &function_entries,
                None,
                Some(candidate),
            );
        }
        match self.compile_aot_non_engine_contract_request(&request, &function_entries, candidate) {
            Ok(result) => result,
            Err(message) => CompileResult::failed(
                request.request_id,
                vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: None,
                    message,
                    path: request.changed_files.first().cloned(),
                    line: None,
                    column: None,
                }],
            ),
        }
    }
}

impl IncrementalCompilerBackend {
    fn runner_diagnostic_from_source(
        &self,
        source: Option<&stasis_compiler::SourceDiagnostic>,
        fallback: String,
        fallback_path: Option<PathBuf>,
    ) -> Diagnostic {
        let Some(source) = source else {
            return Diagnostic {
                severity: DiagnosticSeverity::Error,
                code: None,
                message: fallback,
                path: fallback_path,
                line: None,
                column: None,
            };
        };
        let project_root = self
            .project_root
            .as_ref()
            .map(|root| root.to_string_lossy().to_string());
        let source_entry = self.source_by_path.iter().find(|(path, _)| {
            stasis_compiler::identity::canonical_source_path(project_root.as_deref(), path)
                .is_ok_and(|canonical| canonical == source.path)
        });
        let text = source_entry.map(|(_, text)| text.as_str()).unwrap_or("");
        let start = source.start.min(text.len());
        let line = text[..start].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let column =
            start.saturating_sub(text[..start].rfind('\n').map_or(0, |index| index + 1)) as u32 + 1;
        Diagnostic {
            severity: DiagnosticSeverity::Error,
            code: Some(source.code.as_str().to_string()),
            message: source.message.clone(),
            path: Some(
                source_entry
                    .map(|(path, _)| PathBuf::from(path))
                    .unwrap_or_else(|| PathBuf::from(&source.path)),
            ),
            line: Some(line),
            column: Some(column),
        }
    }

    fn compile_engine_mode_contract_request(
        &mut self,
        request: &CompileRequest,
        include_on_code_swap: bool,
        source_delta: &SourceCacheDelta,
        function_entries: &[EngineFunctionEntry],
        jit_candidate: Option<JitProcess>,
        aot_candidate: Option<AotProcess>,
    ) -> CompileResult {
        let mut aot_linked_image_path: Option<PathBuf> = None;
        let mut aot_linked_image_size_bytes: Option<u64> = None;
        let mut aot_linked_image_sha256: Option<String> = None;
        let mut manifest_rows: Vec<EngineBundleManifestFunctionRow> = Vec::new();
        let mut emitted_function_ids: Option<BTreeSet<u32>> = None;

        match request.target_mode {
            TargetMode::JitDev => {
                let candidate = match jit_candidate {
                    Some(candidate) => candidate,
                    None => match self.compile_jit_candidate_from_cache(source_delta) {
                        Ok(candidate) => candidate,
                        Err(message) => {
                            return CompileResult::failed(
                                request.request_id,
                                vec![Diagnostic {
                                    severity: DiagnosticSeverity::Error,
                                    code: None,
                                    message,
                                    path: request.changed_files.first().cloned(),
                                    line: None,
                                    column: None,
                                }],
                            );
                        }
                    },
                };
                let package = match candidate
                    .build_engine_package(&Self::engine_entrypoints(include_on_code_swap))
                {
                    Ok(package) => package,
                    Err(message) => {
                        return CompileResult::failed(
                            request.request_id,
                            vec![Diagnostic {
                                severity: DiagnosticSeverity::Error,
                                code: None,
                                message,
                                path: request.changed_files.first().cloned(),
                                line: None,
                                column: None,
                            }],
                        );
                    }
                };
                self.last_program_snapshot = candidate.program_snapshot().cloned();
                self.last_jit_engine_package = Some(package);
                self.pending_jit_candidate = Some(candidate);
                if let Some(package) = self.last_jit_engine_package.as_ref() {
                    emitted_function_ids =
                        Some(package.function_code_ptrs.keys().copied().collect());
                }
            }
            TargetMode::AotProd => {
                let mut process = aot_candidate.expect("AOT request has compiled candidate");
                let bundle_output_dir = self
                    .aot_artifact_root
                    .join("engine_bundle")
                    .join(format!("request_{}", request.request_id.0));
                if bundle_output_dir.exists() {
                    if let Err(error) = std::fs::remove_dir_all(&bundle_output_dir) {
                        return CompileResult::failed(request.request_id, vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            code: None,
                            message: format!("failed to clear existing AOT engine bundle directory {}: {error}", bundle_output_dir.display()),
                            path: request.changed_files.first().cloned(), line: None, column: None,
                        }]);
                    }
                }
                let bundle = match process.write_engine_bundle(
                    &Self::engine_entrypoints(include_on_code_swap),
                    &bundle_output_dir,
                ) {
                    Ok(bundle) => bundle,
                    Err(message) => {
                        return CompileResult::failed(
                            request.request_id,
                            vec![Diagnostic {
                                severity: DiagnosticSeverity::Error,
                                code: None,
                                message,
                                path: request.changed_files.first().cloned(),
                                line: None,
                                column: None,
                            }],
                        );
                    }
                };
                self.last_program_snapshot = process.program_snapshot().cloned();
                self.last_aot_engine_bundle = Some(bundle.clone());
                aot_linked_image_path = Some(bundle.manifest_path.clone());
                let metadata = std::fs::metadata(&bundle.manifest_path).map_err(|error| {
                    CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            code: None,
                            message: format!(
                                "failed to stat AOT engine bundle manifest {}: {error}",
                                bundle.manifest_path.display()
                            ),
                            path: request.changed_files.first().cloned(),
                            line: None,
                            column: None,
                        }],
                    )
                });
                match metadata {
                    Ok(meta) => aot_linked_image_size_bytes = Some(meta.len()),
                    Err(result) => return result,
                }
                let digest = compute_file_sha256_hex(&bundle.manifest_path).map_err(|error| {
                    CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            code: None,
                            message: format!(
                                "failed to hash AOT engine bundle manifest {}: {error}",
                                bundle.manifest_path.display()
                            ),
                            path: request.changed_files.first().cloned(),
                            line: None,
                            column: None,
                        }],
                    )
                });
                match digest {
                    Ok(hash) => aot_linked_image_sha256 = Some(hash),
                    Err(result) => return result,
                }
                let manifest = match self.read_engine_bundle_manifest(&bundle.manifest_path) {
                    Ok(manifest) => manifest,
                    Err(message) => {
                        return CompileResult::failed(
                            request.request_id,
                            vec![Diagnostic {
                                severity: DiagnosticSeverity::Error,
                                code: None,
                                message,
                                path: request.changed_files.first().cloned(),
                                line: None,
                                column: None,
                            }],
                        );
                    }
                };
                // Parsed for manifest forward-compatibility, but not used for behavior today.
                let _ = manifest.optimization_profile.as_deref();
                if let Some(literals) = manifest.string_literals.as_ref() {
                    // AOT code references string literals by hashed ID at runtime. Unlike the JIT path,
                    // AOT compilation happens out of band from execution, so the runtime table must be
                    // populated from the bundle manifest before tick/render are called.
                    stasis_dynload::clear_jit_string_literal_table();
                    for literal in literals {
                        stasis_dynload::upsert_jit_string_literal(literal.id, &literal.value);
                    }
                }
                if let Some(entries) = manifest.collection_max_lengths.as_ref() {
                    // Fixed-size arrays/strings rely on .max_length headers stored in the global i32
                    // table. The JIT path seeds these during compilation; AOT must seed them when
                    // the bundle is loaded.
                    for entry in entries {
                        let max_length_path = format!("{}.max_length", entry.path);
                        stasis_dynload::stasis_jit_global_i32_store(
                            crate::hash_global_path(&max_length_path),
                            entry.max_length,
                        );
                    }
                }
                manifest_rows = manifest.functions;
                emitted_function_ids =
                    Some(manifest_rows.iter().map(|row| row.function_id).collect());
            }
        }

        let mut functions = Vec::new();
        let mut hook_fn_id: Option<FnId> = None;
        let mut lifecycle_fn_id_by_name: BTreeMap<String, FnId> = BTreeMap::new();
        for entry in function_entries {
            if let Some(emitted_ids) = emitted_function_ids.as_ref() {
                if !emitted_ids.contains(&entry.fn_id.0) {
                    continue;
                }
            }
            let fn_id = entry.fn_id;
            if is_host_lifecycle_entry(entry) {
                if let Some(previous) = lifecycle_fn_id_by_name.insert(entry.name.clone(), fn_id) {
                    if previous != fn_id {
                        return CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: None,
                    message: format!(
                                "host ABI alias '{}' is ambiguous across canonical function identities",
                                entry.name
                            ),
                            path: None,
                            line: None,
                            column: None,
                        }],
                    );
                    }
                }
            }
            if entry.name == "on_code_swap" {
                hook_fn_id = Some(fn_id);
            }
            functions.push(FunctionPatch { fn_id });
        }
        if functions.is_empty() {
            return CompileResult::failed(
                request.request_id,
                vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: None,
                    message:
                        "engine contract compile produced no emitted function mapping for patch set"
                            .to_string(),
                    path: request.changed_files.first().cloned(),
                    line: None,
                    column: None,
                }],
            );
        }

        let aot_function_symbols = if request.target_mode == TargetMode::AotProd {
            let mut symbol_by_id: BTreeMap<u32, (String, String)> = BTreeMap::new();
            for row in manifest_rows {
                symbol_by_id.insert(row.function_id, (row.symbol_id, row.symbol));
            }
            let mut symbols = Vec::new();
            for entry in function_entries {
                let Some((manifest_symbol_id, symbol)) = symbol_by_id.get(&entry.fn_id.0).cloned()
                else {
                    continue;
                };
                if manifest_symbol_id != entry.symbol_id {
                    return CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            code: None,
                            message: format!(
                                "AOT manifest FnId collision: '{}' vs '{}'",
                                manifest_symbol_id, entry.symbol_id
                            ),
                            path: request.changed_files.first().cloned(),
                            line: None,
                            column: None,
                        }],
                    );
                }
                symbols.push(AotFunctionSymbol {
                    fn_id: entry.fn_id,
                    symbol,
                });
            }
            Some(symbols)
        } else {
            None
        };

        let jit_code_ptr_overrides = if request.target_mode == TargetMode::JitDev {
            let Some(package) = self.last_jit_engine_package.as_ref() else {
                return CompileResult::failed(
                    request.request_id,
                    vec![Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        code: None,
                        message: "missing JIT engine package after successful JIT compile"
                            .to_string(),
                        path: request.changed_files.first().cloned(),
                        line: None,
                        column: None,
                    }],
                );
            };
            let mut overrides = Vec::new();
            for entry in function_entries {
                let Some(code_ptr) = package.function_code_ptrs.get(&entry.fn_id.0).copied() else {
                    continue;
                };
                overrides.push(JitCodePtrOverride {
                    fn_id: entry.fn_id,
                    code_ptr,
                });
            }
            Some(overrides)
        } else {
            None
        };

        let mut result = CompileResult::success_with_host_set_metadata(
            request.request_id,
            self.layout_hash_from_snapshot(),
            FunctionPatchSet { functions },
            request.host_set_id.clone(),
            request.host_set_hash,
            include_on_code_swap.then(|| "on_code_swap".to_string()),
            hook_fn_id,
            aot_linked_image_path,
            aot_linked_image_size_bytes,
            aot_linked_image_sha256,
            aot_function_symbols,
        );
        result.jit_code_ptr_overrides = jit_code_ptr_overrides;
        result
    }

    fn compile_jit_non_engine_contract_request(
        &mut self,
        request: &CompileRequest,
        source_delta: &SourceCacheDelta,
        function_entries: &[EngineFunctionEntry],
        jit_candidate: Option<JitProcess>,
    ) -> Result<CompileResult, String> {
        let candidate = match jit_candidate {
            Some(candidate) => candidate,
            None => self.compile_jit_candidate_from_cache(source_delta)?,
        };
        let function_code_ptrs = candidate.function_code_ptrs();
        self.last_program_snapshot = candidate.program_snapshot().cloned();
        self.pending_jit_candidate = Some(candidate);
        if function_entries.is_empty() || function_code_ptrs.is_empty() {
            return Err(
                "non-engine JIT compile requires at least one parsed function and emitted code pointer"
                    .to_string(),
            );
        }

        let mut functions = Vec::new();
        let mut hook_fn_id: Option<FnId> = None;
        let mut jit_code_ptr_overrides = Vec::new();
        let mut host_aliases = BTreeMap::new();
        for entry in function_entries {
            if !function_code_ptrs.contains_key(&entry.fn_id.0) {
                continue;
            }
            let fn_id = entry.fn_id;
            if is_host_lifecycle_entry(entry)
                && host_aliases.insert(entry.name.clone(), fn_id).is_some()
            {
                return Err(format!("host ABI alias '{}' is ambiguous", entry.name));
            }
            if entry.name == "on_code_swap" {
                if hook_fn_id.is_some_and(|existing| existing != fn_id) {
                    return Err("host ABI alias 'on_code_swap' is ambiguous".to_string());
                }
                hook_fn_id = Some(fn_id);
            }
            functions.push(FunctionPatch { fn_id });
            let code_ptr = function_code_ptrs[&fn_id.0];
            jit_code_ptr_overrides.push(JitCodePtrOverride { fn_id, code_ptr });
        }
        if functions.is_empty() {
            return Err(
                "non-engine JIT compile requires at least one emitted function code pointer"
                    .to_string(),
            );
        }

        let mut result = CompileResult::success_with_host_set_metadata(
            request.request_id,
            self.layout_hash_from_snapshot(),
            FunctionPatchSet { functions },
            request.host_set_id.clone(),
            request.host_set_hash,
            hook_fn_id.map(|_| "on_code_swap".to_string()),
            hook_fn_id,
            None,
            None,
            None,
            None,
        );
        result.jit_code_ptr_overrides = Some(jit_code_ptr_overrides);
        Ok(result)
    }

    fn compile_aot_non_engine_contract_request(
        &mut self,
        request: &CompileRequest,
        function_entries: &[EngineFunctionEntry],
        process: AotProcess,
    ) -> Result<CompileResult, String> {
        let compile =
            self.compile_aot_non_engine_artifacts_from_process(process, request.request_id.0)?;

        let mut functions = Vec::new();
        let mut aot_function_symbols = Vec::new();
        let mut hook_fn_id: Option<FnId> = None;
        for entry in function_entries {
            let Some((symbol, _)) = compile.object_paths_by_function.get(&entry.fn_id.0) else {
                continue;
            };
            let fn_id = entry.fn_id;
            if entry.name == "on_code_swap" {
                if hook_fn_id.is_some_and(|existing| existing != fn_id) {
                    return Err("host ABI alias 'on_code_swap' is ambiguous".to_string());
                }
                hook_fn_id = Some(fn_id);
            }
            functions.push(FunctionPatch { fn_id });
            aot_function_symbols.push(AotFunctionSymbol {
                fn_id,
                symbol: symbol.clone(),
            });
        }
        if functions.is_empty() {
            return Err(
                "non-engine AOT compile requires at least one emitted object artifact".to_string(),
            );
        }

        let result = CompileResult::success_with_host_set_metadata(
            request.request_id,
            self.layout_hash_from_snapshot(),
            FunctionPatchSet { functions },
            request.host_set_id.clone(),
            request.host_set_hash,
            hook_fn_id.map(|_| "on_code_swap".to_string()),
            hook_fn_id,
            compile.linked_image_path,
            compile.linked_image_size_bytes,
            compile.linked_image_sha256,
            Some(aot_function_symbols),
        );
        Ok(result)
    }

    fn refresh_cached_sources(
        &mut self,
        changed_files: &[PathBuf],
    ) -> Result<SourceCacheDelta, String> {
        let mut touched_paths: BTreeSet<String> = BTreeSet::new();
        let mut removed_paths: BTreeSet<String> = BTreeSet::new();
        for path in changed_files {
            let key = path.to_string_lossy().to_string();
            match std::fs::read(path) {
                Ok(bytes) => {
                    let source = String::from_utf8_lossy(&bytes).to_string();
                    self.source_by_path.insert(key.clone(), source);
                    touched_paths.insert(key);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.source_by_path.remove(&key);
                    removed_paths.insert(key);
                }
                Err(error) => {
                    return Err(format!("failed reading {}: {error}", path.display()));
                }
            }
        }
        Ok(SourceCacheDelta {
            touched_paths: touched_paths.into_iter().collect(),
            removed_paths: removed_paths.into_iter().collect(),
        })
    }

    fn sync_jit_process_sources(&mut self, source_delta: &SourceCacheDelta) {
        let requires_full_sync = !self.jit_process_seeded || !source_delta.removed_paths.is_empty();
        if requires_full_sync {
            self.jit_process = JitProcess::new();
            self.jit_process
                .set_project_root(
                    self.project_root
                        .as_ref()
                        .expect("compile root initialized")
                        .to_string_lossy(),
                )
                .expect("validated backend root remains valid");
            for (path, source) in &self.source_by_path {
                self.jit_process.upsert_file(path.clone(), source.clone());
            }
            self.jit_process_seeded = true;
            return;
        }
        for path in &source_delta.touched_paths {
            if let Some(source) = self.source_by_path.get(path) {
                self.jit_process.upsert_file(path.clone(), source.clone());
            }
        }
    }

    fn publish_prepared_jit_candidate(&mut self, request_id: RequestId) -> Result<(), String> {
        let Some(candidate) = self.pending_jit_candidate.take() else {
            return Ok(());
        };
        let Some(sender) = self.prepared_jit_swap_tx.as_ref() else {
            self.jit_process = candidate;
            return Ok(());
        };
        let accepted = self.jit_process.staged_candidate();
        self.jit_process = candidate.staged_candidate();
        sender
            .send(PreparedJitSwap {
                request_id,
                candidate,
            })
            .map_err(|_| {
                self.jit_process = accepted;
                format!(
                    "failed publishing staged JIT candidate for request {}",
                    request_id.0
                )
            })
    }

    fn read_engine_bundle_manifest(&self, path: &Path) -> Result<EngineBundleManifest, String> {
        read_engine_bundle_manifest(path)
    }

    fn layout_hash_from_snapshot(&self) -> LayoutHash {
        LayoutHash(
            self.last_program_snapshot
                .as_ref()
                .expect("successful compiler result must have a program snapshot")
                .layout_digest(),
        )
    }

    fn compile_aot_process_from_source_cache(
        &mut self,
        policy: ReachabilityPolicy,
    ) -> Result<AotProcess, String> {
        let mut process = AotProcess::with_optimization_profile(
            Self::aot_optimization_profile_from_compile_config(&self.aot_compile_config),
        );
        process.set_reachability_policy(policy);
        process.set_target(self.aot_compile_config.target.clone());
        process.set_project_root(
            self.project_root
                .as_ref()
                .ok_or_else(|| "compiler project root is not initialized".to_string())?
                .to_string_lossy(),
        )?;
        for (path, source) in &self.source_by_path {
            process.upsert_file(path.clone(), source.clone());
        }
        if let Err(error) = process.compile() {
            self.last_aot_source_diagnostic = process.last_source_diagnostic().cloned();
            return Err(format!("rust-native AOT compile failed: {error:?}"));
        }
        self.last_aot_source_diagnostic = None;
        Ok(process)
    }

    fn compile_aot_non_engine_artifacts_from_process(
        &mut self,
        mut process: AotProcess,
        request_id: u64,
    ) -> Result<DirectAotArtifactBundle, String> {
        let output_dir = self
            .aot_artifact_root
            .join("non_engine")
            .join(format!("request_{request_id}"));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).map_err(|error| {
                format!(
                    "failed to clear existing AOT object directory {}: {error}",
                    output_dir.display()
                )
            })?;
        }

        let object_dir = output_dir.join("objects");
        let object_paths_by_function = process.write_object_files_by_id(&object_dir)?;
        self.last_program_snapshot = process.program_snapshot().cloned();
        let artifact_paths: Vec<String> = object_paths_by_function
            .values()
            .map(|(_, path)| path.display().to_string())
            .collect();
        let export_symbols: Vec<String> = object_paths_by_function
            .values()
            .map(|(symbol, _)| symbol.clone())
            .collect();
        let mut link_config = self.aot_link_config.clone();
        link_config.target = self.aot_compile_config.target.clone();

        let (linked_image_path, linked_image_size_bytes, linked_image_sha256) =
            if self.enable_aot_link_step && !artifact_paths.is_empty() {
                let linked_output = if cfg!(windows) {
                    output_dir.join("bundle.dll")
                } else if cfg!(target_os = "macos") {
                    output_dir.join("bundle.dylib")
                } else {
                    output_dir.join("bundle.so")
                };
                let object_paths: Vec<PathBuf> = object_paths_by_function
                    .values()
                    .map(|(_, path)| path.clone())
                    .collect();
                link_objects_to_dynamic_library(
                    &object_paths,
                    &linked_output,
                    &export_symbols,
                    &link_config,
                )?;
                let size = std::fs::metadata(&linked_output)
                    .map_err(|error| {
                        format!(
                            "failed to stat linked AOT image {}: {error}",
                            linked_output.display()
                        )
                    })?
                    .len();
                let digest = compute_file_sha256_hex(&linked_output)?;
                (Some(linked_output), Some(size), Some(digest))
            } else {
                (None, None, None)
            };

        self.write_aot_manifest(
            request_id,
            &artifact_paths,
            linked_image_path
                .as_ref()
                .map(|path| path.display().to_string()),
            linked_image_size_bytes,
            linked_image_sha256.clone(),
        )?;

        Ok(DirectAotArtifactBundle {
            output_dir,
            object_paths_by_function,
            linked_image_path,
            linked_image_size_bytes,
            linked_image_sha256,
        })
    }

    fn engine_entrypoints(include_on_code_swap: bool) -> EngineEntrypoints {
        EngineEntrypoints {
            tick: "tick".to_string(),
            render: "render".to_string(),
            on_code_swap: include_on_code_swap.then(|| "on_code_swap".to_string()),
        }
    }

    fn aot_optimization_profile_from_compile_config(
        config: &AotCompileConfig,
    ) -> AotOptimizationProfile {
        match config.opt_level.as_str() {
            "none" => AotOptimizationProfile::None,
            "speed_and_size" => AotOptimizationProfile::SpeedAndSize,
            "speed" => AotOptimizationProfile::Speed,
            _ => AotOptimizationProfile::SpeedAndSize,
        }
    }

    #[cfg(test)]
    fn last_jit_engine_package(&self) -> Option<&JitEnginePackage> {
        self.last_jit_engine_package.as_ref()
    }

    #[cfg(test)]
    fn jit_artifact_slot_for_function_name(&self, name: &str) -> Option<u32> {
        self.jit_process.artifact_slot_for_function_name(name)
    }

    #[cfg(test)]
    fn jit_generation_source_revision(&self) -> Option<u64> {
        self.jit_process
            .generation_metadata()
            .map(|metadata| metadata.source_revision)
    }

    #[cfg(test)]
    fn last_aot_engine_bundle(&self) -> Option<&AotEngineBundle> {
        self.last_aot_engine_bundle.as_ref()
    }

    fn write_aot_manifest(
        &self,
        request_id: u64,
        artifact_paths: &[String],
        linked_image_path: Option<String>,
        linked_image_size_bytes: Option<u64>,
        linked_image_sha256: Option<String>,
    ) -> Result<(), String> {
        let manifest_path = self.aot_artifact_root.join("last_patch_manifest.json");
        let manifest = AotPatchManifest {
            request_id,
            artifact_paths: artifact_paths.to_vec(),
            linked_image_path,
            linked_image_size_bytes,
            linked_image_sha256,
        };
        let payload = serde_json::to_string_pretty(&manifest)
            .map_err(|error| format!("failed to serialize AOT manifest: {error}"))?;
        std::fs::write(&manifest_path, payload).map_err(|error| {
            format!(
                "failed to write AOT manifest {}: {error}",
                manifest_path.display()
            )
        })?;
        Ok(())
    }
}

#[allow(dead_code)]
fn expand_layout_hash(layout_hash: i32) -> LayoutHash {
    let as_u32 = layout_hash as u32;
    let mut out = [0u8; 32];
    out[0..4].copy_from_slice(&as_u32.to_le_bytes());
    out[4..8].copy_from_slice(&(as_u32.rotate_left(13)).to_le_bytes());
    out[8..12].copy_from_slice(&(as_u32.rotate_left(21)).to_le_bytes());
    out[12..16].copy_from_slice(&(as_u32.rotate_left(5)).to_le_bytes());
    out[16..20].copy_from_slice(&(as_u32.wrapping_mul(16777619)).to_le_bytes());
    out[20..24].copy_from_slice(&(as_u32 ^ 0x9e3779b9).to_le_bytes());
    out[24..28].copy_from_slice(&(as_u32.wrapping_add(0x85ebca6b)).to_le_bytes());
    out[28..32].copy_from_slice(&(as_u32 ^ 0xc2b2ae35).to_le_bytes());
    LayoutHash(out)
}

fn compute_file_sha256_hex(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

const SELF_HOST_RUNTIME_CMAKE: &str = "runtime/CMakeLists.txt";
const SELF_HOST_MOBILE_MAIN: &str = "mobile/shells/common/stasis_mobile_main.c";

fn self_host_inputs_are_complete(root: &Path) -> bool {
    root.join(SELF_HOST_RUNTIME_CMAKE).is_file() && root.join(SELF_HOST_MOBILE_MAIN).is_file()
}

fn resolve_self_host_repo_root(
    executable_path: Option<&Path>,
    compile_time_root: &Path,
) -> Result<PathBuf, String> {
    let mut candidate_roots = Vec::with_capacity(3);
    if let Some(executable_directory) = executable_path.and_then(Path::parent) {
        candidate_roots.push(executable_directory.to_path_buf());
        if executable_directory.file_name() == Some(std::ffi::OsStr::new("bin")) {
            if let Some(bundle_root) = executable_directory.parent() {
                candidate_roots.push(bundle_root.to_path_buf());
            }
        }
    }
    if !candidate_roots
        .iter()
        .any(|candidate| candidate == compile_time_root)
    {
        candidate_roots.push(compile_time_root.to_path_buf());
    }

    for candidate in &candidate_roots {
        if self_host_inputs_are_complete(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }

    let checked_roots = candidate_roots
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "unable to locate Stasis self-host repository inputs; checked root locations: {}. Expected regular files {} and {}",
        checked_roots,
        SELF_HOST_RUNTIME_CMAKE,
        SELF_HOST_MOBILE_MAIN,
    ))
}

fn self_host_repo_root() -> Result<PathBuf, String> {
    let compile_time_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    resolve_self_host_repo_root(std::env::current_exe().ok().as_deref(), &compile_time_root)
}

fn resolve_self_host_aot_entry_file(
    project_dir: &Path,
    entry_file_override: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let Some(entry_path) = entry_file_override.map(PathBuf::from) else {
        return Ok(None);
    };
    let full_path = if entry_path.is_absolute() {
        entry_path
    } else {
        project_dir.join(entry_path)
    };
    let canonical = full_path.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize AOT entry file {}: {error}",
            full_path.display()
        )
    })?;
    Ok(Some(canonical))
}

fn resolve_latest_existing_path(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .filter(|path| path.exists())
        .max_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
}

fn resolve_stasis_dynload_lib() -> Option<PathBuf> {
    let installed_name = if cfg!(windows) {
        "stasis_dynload.dll.lib"
    } else {
        "libstasis_dynload.a"
    };
    if let Some(installed) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(installed_name)))
        .filter(|path| path.is_file())
    {
        return Some(installed);
    }
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| self_host_repo_root().ok().map(|root| root.join("target")))?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    let link_lib_names: &[&str] = if cfg!(windows) {
        &["stasis_dynload.dll.lib"]
    } else {
        &["libstasis_dynload.a", "stasis_dynload.a"]
    };

    for profile in ["debug", "release"] {
        let base = target_dir.join(profile);
        for name in link_lib_names {
            let direct = base.join(name);
            if direct.exists() {
                candidates.push(direct);
            }
        }

        let deps = base.join("deps");
        let Ok(entries) = std::fs::read_dir(&deps) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if cfg!(windows) {
                if name.starts_with("stasis_dynload-") && name.ends_with(".dll.lib") {
                    candidates.push(path);
                }
            } else if (name.starts_with("libstasis_dynload-")
                || name.starts_with("stasis_dynload-"))
                && name.ends_with(".a")
            {
                candidates.push(path);
            }
        }
    }

    resolve_latest_existing_path(candidates)
}

fn runtime_runner_file_name() -> &'static str {
    if cfg!(windows) {
        "stasis_runner.exe"
    } else {
        "stasis_runner"
    }
}

fn append_runtime_runner_candidates(candidates: &mut Vec<PathBuf>, directory: &Path) {
    if cfg!(target_os = "macos") {
        candidates.push(
            directory
                .join("stasis_runner.app")
                .join("Contents")
                .join("MacOS")
                .join("stasis_runner"),
        );
    }
    candidates.push(directory.join(runtime_runner_file_name()));
}

fn runtime_graphics_file_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["stasis_graphics.dll"]
    } else if cfg!(target_os = "macos") {
        &["libstasis_graphics.dylib", "stasis_graphics.dylib"]
    } else {
        &["libstasis_graphics.so", "stasis_graphics.so"]
    }
}

fn packaged_runtime_library_extension() -> &'static str {
    if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

fn runtime_bridge_object_extension(target: &stasis_jit::AotTarget) -> &'static str {
    if matches!(target, stasis_jit::AotTarget::Native) && cfg!(windows) {
        "obj"
    } else {
        "o"
    }
}

fn should_link_stasis_dynload(target: &stasis_jit::AotTarget) -> bool {
    matches!(target, stasis_jit::AotTarget::Native)
}

fn default_runtime_bridge_compiler(target: &stasis_jit::AotTarget) -> PathBuf {
    if target.is_android() {
        PathBuf::from("clang")
    } else if cfg!(windows) {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("clang-cl.exe")))
            .filter(|path| path.is_file())
            .or_else(resolve_host_clang_cl)
            .unwrap_or_else(|| PathBuf::from("clang-cl.exe"))
    } else {
        PathBuf::from("cc")
    }
}

#[cfg(windows)]
fn resolve_host_clang_cl() -> Option<PathBuf> {
    ["BuildTools", "Enterprise", "Community", "Professional"]
        .into_iter()
        .map(|edition| {
            PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\2022")
                .join(edition)
                .join("VC/Tools/Llvm/x64/bin/clang-cl.exe")
        })
        .find(|path| path.is_file())
}

#[cfg(not(windows))]
fn resolve_host_clang_cl() -> Option<PathBuf> {
    None
}

fn ensure_stasis_dynload_link_library() -> Result<PathBuf, String> {
    if let Some(existing) = resolve_stasis_dynload_lib() {
        return Ok(existing);
    }
    let repo_root = self_host_repo_root()?;
    let mut command = std::process::Command::new("cargo");
    if cfg!(windows) {
        command.arg("build").arg("-p").arg("stasis_dynload");
    } else {
        command.arg("rustc").arg("-p").arg("stasis_dynload");
    }
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    if !cfg!(windows) {
        command.arg("--").arg("--crate-type").arg("staticlib");
    }
    command.current_dir(&repo_root);
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        command.env("CARGO_TARGET_DIR", target_dir);
    }

    let output = command.output().map_err(|error| {
        format!("failed to spawn Cargo for the stasis_dynload link library: {error}")
    })?;
    if !output.status.success() {
        return Err(format!(
            "failed to build stasis_dynload link library\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    resolve_stasis_dynload_lib().ok_or_else(|| {
        "stasis_dynload build reported success but no link library was found".to_string()
    })
}

#[cfg(windows)]
fn stage_stasis_dynload_runtime(link_library: &Path, output: &Path) -> Result<(), String> {
    let file_name = link_library
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "invalid stasis_dynload link library {}",
                link_library.display()
            )
        })?;
    let dll_name = file_name
        .strip_suffix(".lib")
        .ok_or_else(|| format!("expected a .lib import library, got {file_name}"))?;
    let dll = link_library.with_file_name(dll_name);
    if !dll.is_file() {
        return Err(format!(
            "stasis_dynload runtime DLL is missing: {}",
            dll.display()
        ));
    }
    let destination = output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(dll_name);
    copy_file_creating_parent(&dll, &destination)
}

#[cfg(not(windows))]
fn stage_stasis_dynload_runtime(_link_library: &Path, _output: &Path) -> Result<(), String> {
    Ok(())
}

fn resolve_runtime_runner_path(repo_root: &Path) -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_RUNNER_PATH") {
        let configured = PathBuf::from(configured);
        if configured.is_file() {
            return Some(configured);
        }
    }
    let mut candidates = Vec::new();
    if let Some(installed_directory) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        append_runtime_runner_candidates(&mut candidates, &installed_directory);
    }
    for directory in [
        repo_root.to_path_buf(),
        repo_root.join("build"),
        repo_root.join("runtime").join("build").join("bin"),
        repo_root
            .join("runtime")
            .join("build")
            .join("bin")
            .join("Release"),
    ] {
        append_runtime_runner_candidates(&mut candidates, &directory);
    }
    resolve_latest_existing_path(candidates)
}

fn resolve_runtime_graphics_path(repo_root: &Path) -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_LIBRARY_PATH") {
        let configured = PathBuf::from(configured);
        if configured.is_file() {
            return Some(configured);
        }
    }
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_DLL_PATH") {
        let configured = PathBuf::from(configured);
        if configured.is_file() {
            return Some(configured);
        }
    }
    let mut candidates = Vec::new();
    for name in runtime_graphics_file_names() {
        if let Some(installed) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join(name)))
        {
            candidates.push(installed);
        }
        candidates.push(repo_root.join(name));
        candidates.push(repo_root.join("build").join(name));
        candidates.push(
            repo_root
                .join("runtime")
                .join("build")
                .join("bin")
                .join("Release")
                .join(name),
        );
        candidates.push(
            repo_root
                .join("runtime")
                .join("build")
                .join("bin")
                .join(name),
        );
    }
    resolve_latest_existing_path(candidates)
}

fn resolve_msvc_link_exe() -> Option<PathBuf> {
    let roots = [
        PathBuf::from(
            r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC",
        ),
        PathBuf::from(r"C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC"),
    ];
    let mut candidates = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry
                .path()
                .join("bin")
                .join("HostX64")
                .join("x64")
                .join("link.exe");
            if path.exists() {
                candidates.push(path);
            }
        }
    }
    resolve_latest_existing_path(candidates)
}

fn resolve_rust_lld_exe() -> Option<PathBuf> {
    let toolchain = std::env::var_os("RUSTUP_TOOLCHAIN")
        .unwrap_or_else(|| "stable-x86_64-pc-windows-msvc".into());
    let candidate = PathBuf::from(std::env::var_os("USERPROFILE")?)
        .join(".rustup")
        .join("toolchains")
        .join(toolchain)
        .join("lib")
        .join("rustlib")
        .join("x86_64-pc-windows-msvc")
        .join("bin")
        .join("rust-lld.exe");
    candidate.exists().then_some(candidate)
}

fn resolve_installed_lld_link() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join("lld-link.exe");
    path.is_file().then_some(path)
}

fn ensure_rust_lld_link_wrapper(artifact_root: &Path) -> Option<PathBuf> {
    let rust_lld = resolve_rust_lld_exe()?;
    std::fs::create_dir_all(artifact_root).ok()?;
    let wrapper_path = artifact_root.join("rust-lld-link.cmd");
    let script = format!(
        "@echo off\r\n\"{}\" -flavor link %*\r\n",
        rust_lld.display()
    );
    std::fs::write(&wrapper_path, script).ok()?;
    Some(wrapper_path)
}

fn ensure_runtime_release_artifacts() -> Result<(PathBuf, PathBuf), String> {
    let repo_root = self_host_repo_root()?;
    let runner = resolve_runtime_runner_path(&repo_root);
    let graphics = resolve_runtime_graphics_path(&repo_root);
    if let (Some(runner), Some(graphics)) = (runner, graphics) {
        return Ok((runner, graphics));
    }

    if cfg!(windows) {
        let output = std::process::Command::new("cmd")
            .arg("/c")
            .arg("runtime\\build.bat")
            .current_dir(&repo_root)
            .output()
            .map_err(|error| format!("failed to spawn runtime\\build.bat: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "runtime\\build.bat failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    } else {
        let configure = std::process::Command::new("cmake")
            .arg("-S")
            .arg("runtime")
            .arg("-B")
            .arg("runtime/build")
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .current_dir(&repo_root)
            .output()
            .map_err(|error| format!("failed to spawn cmake configure for runtime: {error}"))?;
        if !configure.status.success() {
            return Err(format!(
                "cmake runtime configure failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&configure.stdout),
                String::from_utf8_lossy(&configure.stderr)
            ));
        }

        let build = std::process::Command::new("cmake")
            .arg("--build")
            .arg("runtime/build")
            .arg("--config")
            .arg("Release")
            .current_dir(&repo_root)
            .output()
            .map_err(|error| format!("failed to spawn cmake build for runtime: {error}"))?;
        if !build.status.success() {
            return Err(format!(
                "cmake runtime build failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&build.stdout),
                String::from_utf8_lossy(&build.stderr)
            ));
        }
    }

    let runner = resolve_runtime_runner_path(&repo_root).ok_or_else(|| {
        format!(
            "runtime build succeeded but {} was not found",
            runtime_runner_file_name()
        )
    })?;
    let graphics = resolve_runtime_graphics_path(&repo_root).ok_or_else(|| {
        format!(
            "runtime build succeeded but none of {:?} were found",
            runtime_graphics_file_names()
        )
    })?;
    Ok((runner, graphics))
}

fn copy_file_creating_parent(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create parent directory {}: {error}",
                parent.display()
            )
        })?;
    }
    std::fs::copy(src, dst).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|error| format!("failed to create directory {}: {error}", dst.display()))?;
    let entries = std::fs::read_dir(src)
        .map_err(|error| format!("failed to read directory {}: {error}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read directory entry in {}: {error}",
                src.display()
            )
        })?;
        let path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to read file type for {}: {error}", path.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&path, &dst_path)?;
        } else if file_type.is_file() {
            copy_file_creating_parent(&path, &dst_path)?;
        }
    }
    Ok(())
}

fn collect_struct_meta_fields(root: &Path) -> Result<Vec<PackagedRuntimeField>, String> {
    fn json_value_by_path<'a>(
        root: &'a serde_json::Value,
        path: &str,
    ) -> Option<&'a serde_json::Value> {
        if path.is_empty() {
            return Some(root);
        }
        let mut value = root;
        for segment in path.split('.') {
            value = value.get(segment)?;
        }
        Some(value)
    }

    fn walk(dir: &Path, out: &mut BTreeMap<String, PackagedRuntimeField>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to read directory entry in {}: {error}",
                    dir.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                format!("failed to read file type for {}: {error}", path.display())
            })?;
            if file_type.is_dir() {
                walk(&path, out)?;
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".struct-meta.json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let meta: StructMetaExportFile = serde_json::from_str(&text)
                .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
            if let Some(table) = &meta.csv_table {
                if table.rows_path.is_empty()
                    || table.row_count_path.is_empty()
                    || table.rows_path.contains('.')
                    || table.row_count_path.contains('.')
                    || table.capacity == 0
                    || table.key_columns.is_empty()
                {
                    return Err(format!("invalid csvTable schema in {}", path.display()));
                }
                let prefix = format!("{}.", table.rows_path);
                let mut columns = BTreeSet::new();
                for field in &meta.fields {
                    let suffix = field.json_path.strip_prefix(&prefix).ok_or_else(|| {
                        format!(
                            "CSV table target {} in {} must be below rowsPath {}",
                            field.json_path,
                            path.display(),
                            table.rows_path
                        )
                    })?;
                    if suffix.is_empty()
                        || suffix.contains('.')
                        || field.array_count != table.capacity
                    {
                        return Err(format!(
                            "invalid CSV table target {} in {}",
                            field.json_path,
                            path.display()
                        ));
                    }
                    let column = field.csv_column.as_deref().unwrap_or(&field.json_path);
                    if column.is_empty() || column.contains('.') || !columns.insert(column) {
                        return Err(format!(
                            "invalid or duplicate CSV table column {column} in {}",
                            path.display()
                        ));
                    }
                }
                let mut keys = BTreeSet::new();
                for key in &table.key_columns {
                    if !keys.insert(key) {
                        return Err(format!(
                            "duplicate CSV table key column {key} in {}",
                            path.display()
                        ));
                    }
                    if !columns.contains(key.as_str()) {
                        return Err(format!(
                            "CSV table key column {key} in {} has no target field",
                            path.display()
                        ));
                    }
                }
            }
            let data_name = name
                .strip_suffix(".struct-meta.json")
                .expect("metadata suffix checked");
            let json_path = path.with_file_name(format!("{data_name}.json"));
            let csv_path = path.with_file_name(format!("{data_name}.csv"));
            if json_path.is_file() && csv_path.is_file() {
                return Err(format!(
                    "data files {} and {} cannot share metadata {}",
                    json_path.display(),
                    csv_path.display(),
                    path.display()
                ));
            }
            let data_path = if json_path.is_file() {
                Some(json_path)
            } else if csv_path.is_file() {
                Some(csv_path)
            } else {
                None
            };
            let data_root = if let Some(data_path) = data_path.as_ref() {
                let data_text = std::fs::read_to_string(&data_path)
                    .map_err(|error| format!("failed to read {}: {error}", data_path.display()))?;
                if data_path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
                {
                    let fields: Vec<crate::CsvBindingField> = meta
                        .fields
                        .iter()
                        .map(|field| crate::CsvBindingField {
                            path: field.json_path.clone(),
                            csv_column: field.csv_column.clone(),
                            type_name: field.field_type.clone(),
                            array_count: field.array_count,
                        })
                        .collect();
                    Some(
                        if let Some(table) = &meta.csv_table {
                            crate::parse_csv_table_binding(&data_text, &fields, table)
                        } else {
                            crate::parse_flat_csv_binding(&data_text, &fields)
                        }
                        .map_err(|error| {
                            format!("failed to parse {}: {error}", data_path.display())
                        })?,
                    )
                } else {
                    if meta.csv_table.is_some() {
                        return Err(format!(
                            "csvTable metadata requires a CSV data file: {}",
                            data_path.display()
                        ));
                    }
                    Some(
                        serde_json::from_str::<serde_json::Value>(&data_text).map_err(|error| {
                            format!("failed to parse {}: {error}", data_path.display())
                        })?,
                    )
                }
            } else {
                None
            };
            if let Some(root) = data_root.as_ref() {
                let mut paths: Vec<String> = meta
                    .fields
                    .iter()
                    .map(|field| field.json_path.clone())
                    .collect();
                if let Some(table) = &meta.csv_table {
                    paths.push(table.row_count_path.clone());
                }
                crate::validate_binding_source_paths(root, &paths).map_err(|error| {
                    format!(
                        "data file {} does not match target metadata: {error}",
                        data_path
                            .as_ref()
                            .expect("data root requires data path")
                            .display()
                    )
                })?;
            }
            let csv_table = meta.csv_table.clone();
            for field in meta.fields {
                let field_name = match field.name {
                    Some(name) if !name.is_empty() => name,
                    _ if !meta.global_name.is_empty() => {
                        let path_suffix = field.json_path.replace('.', "__");
                        if path_suffix.is_empty() {
                            meta.global_name.clone()
                        } else {
                            format!("{}__{path_suffix}", meta.global_name)
                        }
                    }
                    _ => {
                        return Err(format!(
                            "metadata field in {} requires name or globalName",
                            path.display()
                        ));
                    }
                };
                let initial_value = data_root
                    .as_ref()
                    .and_then(|root| json_value_by_path(root, &field.json_path))
                    .cloned();
                if data_root.is_some() && initial_value.is_none() {
                    return Err(format!(
                        "data file {} is missing metadata path {}",
                        data_path
                            .as_ref()
                            .expect("data root requires data path")
                            .display(),
                        field.json_path
                    ));
                }
                let collection_field = if let Some(table) = &csv_table {
                    Some(
                        field
                            .json_path
                            .strip_prefix(&format!("{}.", table.rows_path))
                            .ok_or_else(|| {
                                format!(
                                    "CSV table target {} in {} must be below rowsPath {}",
                                    field.json_path,
                                    path.display(),
                                    table.rows_path
                                )
                            })?
                            .to_string(),
                    )
                } else {
                    None
                };
                let next = PackagedRuntimeField {
                    name: field_name.clone(),
                    size: field.size,
                    field_type: field.field_type,
                    array_count: field.array_count,
                    initial_value,
                    collection_path: csv_table
                        .as_ref()
                        .map(|table| format!("{}.{}", meta.global_name, table.rows_path)),
                    collection_field,
                    runtime_path: None,
                };
                if let Some(existing) = out.get(&field_name) {
                    if existing != &next {
                        return Err(format!(
                            "conflicting packaged data values for runtime field {field_name}"
                        ));
                    }
                } else {
                    out.insert(field_name, next);
                }
            }
            if let Some(table) = csv_table {
                let path_suffix = table.row_count_path.replace('.', "__");
                let field_name = format!("{}__{path_suffix}", meta.global_name);
                let initial_value = data_root
                    .as_ref()
                    .and_then(|root| json_value_by_path(root, &table.row_count_path))
                    .cloned();
                let next = PackagedRuntimeField {
                    name: field_name.clone(),
                    size: 4,
                    field_type: "i32".to_string(),
                    array_count: 1,
                    initial_value,
                    collection_path: None,
                    collection_field: None,
                    runtime_path: None,
                };
                if let Some(existing) = out.get(&field_name) {
                    if existing != &next {
                        return Err(format!(
                            "conflicting packaged data values for runtime field {field_name}"
                        ));
                    }
                } else {
                    out.insert(field_name, next);
                }
            }
        }
        Ok(())
    }

    let mut fields = BTreeMap::new();
    if root.exists() {
        walk(root, &mut fields)?;
    }
    Ok(fields.into_values().collect())
}

fn is_bundleable_asset_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "json" | "csv" | "svg" | "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    )
}

fn copy_json_referenced_absolute_assets(
    value: &mut serde_json::Value,
    staged_entry_dir: &Path,
    package_rel_root: &str,
    copied_assets: &mut BTreeMap<PathBuf, String>,
) -> Result<bool, String> {
    match value {
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for child in map.values_mut() {
                changed |= copy_json_referenced_absolute_assets(
                    child,
                    staged_entry_dir,
                    package_rel_root,
                    copied_assets,
                )?;
            }
            Ok(changed)
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for child in items {
                changed |= copy_json_referenced_absolute_assets(
                    child,
                    staged_entry_dir,
                    package_rel_root,
                    copied_assets,
                )?;
            }
            Ok(changed)
        }
        serde_json::Value::String(text) => {
            let source = PathBuf::from(text.as_str());
            if !source.is_absolute() || !is_bundleable_asset_extension(&source) {
                return Ok(false);
            }
            if !source.is_file() {
                return Err(format!(
                    "referenced absolute package asset is unavailable: {}",
                    source.display()
                ));
            }
            let source = source.canonicalize().unwrap_or(source);

            if let Some(existing) = copied_assets.get(&source) {
                *text = existing.clone();
                return Ok(true);
            }

            let asset_dir = staged_entry_dir.join("assets").join("external");
            std::fs::create_dir_all(&asset_dir).map_err(|error| {
                format!(
                    "failed to create asset directory {}: {error}",
                    asset_dir.display()
                )
            })?;

            let file_name = source
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("invalid asset file name {}", source.display()))?;
            let bytes = std::fs::read(&source)
                .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
            let digest = format!("{:x}", Sha256::digest(&bytes));
            let destination = asset_dir.join(format!("{digest}-{file_name}"));
            if !destination.exists() {
                std::fs::write(&destination, &bytes).map_err(|error| {
                    format!("failed to write {}: {error}", destination.display())
                })?;
            }
            let rel_name = destination
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("invalid packaged asset path {}", destination.display()))?;
            let rel_path = format!("{package_rel_root}/assets/external/{rel_name}");
            copied_assets.insert(source, rel_path.clone());
            *text = rel_path;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub fn rewrite_packaged_json_asset_paths(
    staged_entry_dir: &Path,
    package_rel_root: &str,
) -> Result<(), String> {
    fn walk(
        dir: &Path,
        staged_entry_dir: &Path,
        package_rel_root: &str,
        copied_assets: &mut BTreeMap<PathBuf, String>,
    ) -> Result<bool, String> {
        let mut entries = std::fs::read_dir(dir)
            .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?;
        let mut entries = entries
            .by_ref()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!(
                    "failed to read directory entry in {}: {error}",
                    dir.display()
                )
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut changed = false;
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                format!("failed to read file type for {}: {error}", path.display())
            })?;
            if file_type.is_dir() {
                changed |= walk(&path, staged_entry_dir, package_rel_root, copied_assets)?;
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let lowercase_name = name.to_ascii_lowercase();
            if !lowercase_name.ends_with(".json") || lowercase_name.ends_with(".struct-meta.json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
            let mut value: serde_json::Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if copy_json_referenced_absolute_assets(
                &mut value,
                staged_entry_dir,
                package_rel_root,
                copied_assets,
            )? {
                changed = true;
                let next = serde_json::to_string_pretty(&value)
                    .map_err(|error| format!("failed to serialize {}: {error}", path.display()))?;
                std::fs::write(&path, next)
                    .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
            }
        }
        Ok(changed)
    }

    let mut copied_assets = BTreeMap::new();
    if staged_entry_dir.exists() {
        while walk(
            staged_entry_dir,
            staged_entry_dir,
            package_rel_root,
            &mut copied_assets,
        )? {}
    }
    Ok(())
}

fn stage_entry_support_files(
    project_dir: &Path,
    entry_file: Option<&Path>,
    output_root: &Path,
) -> Result<PackagedAotSupportFiles, String> {
    let Some(entry_file) = entry_file else {
        return Ok(PackagedAotSupportFiles::default());
    };
    let entry_root = entry_file.parent().unwrap_or(project_dir);
    let package_dir_name = entry_file
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("invalid entry file name {}", entry_file.display()))?
        .to_string();
    let source_bundle_dir = entry_root.join(&package_dir_name);
    let staged_bundle_dir = output_root.join(&package_dir_name);
    let mut staged_roots = Vec::new();
    if source_bundle_dir.exists() {
        if staged_bundle_dir.exists() {
            std::fs::remove_dir_all(&staged_bundle_dir).map_err(|error| {
                format!(
                    "failed to clear existing staged asset directory {}: {error}",
                    staged_bundle_dir.display()
                )
            })?;
        }
        copy_dir_recursive(&source_bundle_dir, &staged_bundle_dir)?;
        rewrite_packaged_json_asset_paths(&staged_bundle_dir, &package_dir_name)?;
        staged_roots.push(staged_bundle_dir.clone());
    }

    let project_data_dir = project_dir.join("data");
    let staged_data_dir = output_root.join("data");
    if project_data_dir.exists() {
        let source_data_abs = project_data_dir
            .canonicalize()
            .unwrap_or_else(|_| project_data_dir.clone());
        let staged_data_abs = staged_data_dir
            .canonicalize()
            .unwrap_or_else(|_| staged_data_dir.clone());
        if source_data_abs == staged_data_abs {
            staged_roots.push(project_data_dir);
        } else {
            if staged_data_dir.exists() {
                std::fs::remove_dir_all(&staged_data_dir).map_err(|error| {
                    format!(
                        "failed to clear existing staged data directory {}: {error}",
                        staged_data_dir.display()
                    )
                })?;
            }
            copy_dir_recursive(&project_data_dir, &staged_data_dir)?;
            rewrite_packaged_json_asset_paths(&staged_data_dir, "data")?;
            staged_roots.push(staged_data_dir);
        }
    }

    let mut fields_by_name: BTreeMap<String, PackagedRuntimeField> = BTreeMap::new();
    for root in staged_roots {
        for field in collect_struct_meta_fields(&root)? {
            if let Some(existing) = fields_by_name.get(&field.name) {
                if existing != &field {
                    return Err(format!(
                        "conflicting packaged data values for runtime field {}",
                        field.name
                    ));
                }
            } else {
                fields_by_name.insert(field.name.clone(), field);
            }
        }
    }
    let runtime_fields: Vec<_> = fields_by_name.into_values().collect();
    let mut support = PackagedAotSupportFiles {
        runtime_fields,
        ..PackagedAotSupportFiles::default()
    };

    let data_json = staged_bundle_dir.join("data").join("config.json");
    let data_meta = staged_bundle_dir
        .join("data")
        .join("config.struct-meta.json");
    if data_json.exists() && data_meta.exists() {
        support.data_bind_json_rel = Some(format!("{package_dir_name}/data/config.json"));
        support.data_bind_meta_rel =
            Some(format!("{package_dir_name}/data/config.struct-meta.json"));
    }

    Ok(support)
}

fn state_layout_runtime_fields(
    layout: &StateLayout,
    include_bridge_owned: bool,
) -> Result<Vec<PackagedRuntimeField>, String> {
    fn field_width(type_name: &str) -> Result<usize, String> {
        match type_name {
            "bool" | "u32" | "i32" | "f32" => Ok(4),
            "u8" => Ok(1),
            "u16" => Ok(2),
            "f64" => Ok(8),
            other => Err(format!("unsupported AOT state storage type '{other}'")),
        }
    }

    fn is_bridge_owned(path: &str) -> bool {
        matches!(
            path,
            "host_i32"
                | "host_f32"
                | "gfx_cmd_i32"
                | "gfx_cmd_f32"
                | "gfx_cmd_u8"
                | "host_req_seq"
                | "host_req_flags"
                | "host_req_window_w_px"
                | "host_req_window_h_px"
        )
    }

    let collection_capacities = layout
        .collections
        .iter()
        .map(|collection| (collection.path.as_str(), collection.capacity))
        .collect::<BTreeMap<_, _>>();
    let mut fields = Vec::new();
    for scalar in &layout.scalars {
        if !include_bridge_owned && is_bridge_owned(&scalar.path) {
            continue;
        }
        let initial_value = scalar.path.strip_suffix(".max_length").and_then(|parent| {
            collection_capacities
                .get(parent)
                .map(|capacity| serde_json::json!(capacity))
        });
        let storage_type_name = scalar.storage_type_name();
        fields.push(PackagedRuntimeField {
            name: aot_storage_symbol(AotStorageSymbolKind::Scalar, &scalar.path, ""),
            size: field_width(storage_type_name)?,
            field_type: storage_type_name.to_string(),
            array_count: 1,
            initial_value,
            collection_path: None,
            collection_field: None,
            runtime_path: Some(scalar.path.clone()),
        });
    }
    for collection in &layout.collections {
        if !include_bridge_owned && is_bridge_owned(&collection.path) {
            continue;
        }
        let array_count = usize::try_from(collection.capacity).map_err(|_| {
            format!(
                "negative AOT state collection capacity {} for '{}'",
                collection.capacity, collection.path
            )
        })?;
        for field in &collection.fields {
            let storage_type_name = field.storage_type_name();
            let width = field_width(storage_type_name)?;
            let name =
                aot_storage_symbol(AotStorageSymbolKind::Array, &collection.path, &field.field);
            fields.push(PackagedRuntimeField {
                name,
                size: width.checked_mul(array_count).ok_or_else(|| {
                    format!("AOT state storage size overflow for '{}'", collection.path)
                })?,
                field_type: storage_type_name.to_string(),
                array_count,
                initial_value: None,
                collection_path: Some(collection.path.clone()),
                collection_field: (!field.field.is_empty()).then(|| field.field.clone()),
                runtime_path: Some(collection.path.clone()),
            });
        }
    }
    Ok(fields)
}

fn merge_runtime_fields(
    layout: &StateLayout,
    support_fields: &[PackagedRuntimeField],
) -> Result<Vec<PackagedRuntimeField>, String> {
    let mut fields = state_layout_runtime_fields(layout, false)?
        .into_iter()
        .map(|field| (field.name.clone(), field))
        .collect::<BTreeMap<_, _>>();
    for field in support_fields {
        fields.insert(field.name.clone(), field.clone());
    }
    Ok(fields.into_values().collect())
}

pub fn build_aot_direct_storage_source(
    layout: &StateLayout,
) -> Result<(String, Vec<String>), String> {
    let mut source = String::from(
        "#ifndef STASIS_EXPORT\n\
#if defined(_WIN32)\n\
#define STASIS_EXPORT __declspec(dllexport)\n\
#else\n\
#define STASIS_EXPORT __attribute__((visibility(\"default\")))\n\
#endif\n\
#endif\n",
    );
    let mut register_lines = Vec::new();
    for field in state_layout_runtime_fields(layout, true)? {
        append_runtime_bridge_field_source(&mut source, &mut register_lines, &field)?;
    }
    Ok((source, register_lines))
}

fn append_runtime_bridge_field_source(
    source: &mut String,
    register_lines: &mut Vec<String>,
    field: &PackagedRuntimeField,
) -> Result<(), String> {
    fn values_for_field<'a>(
        field: &'a PackagedRuntimeField,
    ) -> Result<Vec<&'a serde_json::Value>, String> {
        let Some(value) = field.initial_value.as_ref() else {
            return Ok(Vec::new());
        };
        if field.array_count > 1 {
            let values = value.as_array().ok_or_else(|| {
                format!("packaged data field {} must be a JSON array", field.name)
            })?;
            if values.len() != field.array_count {
                return Err(format!(
                    "packaged data field {} requires {} values, found {}",
                    field.name,
                    field.array_count,
                    values.len()
                ));
            }
            Ok(values.iter().collect())
        } else {
            Ok(vec![value])
        }
    }

    fn i32_literal(value: &serde_json::Value, field_name: &str) -> Result<String, String> {
        if let Some(value) = value.as_bool() {
            return Ok(if value { "1" } else { "0" }.to_string());
        }
        let value = value.as_i64().ok_or_else(|| {
            format!("packaged data field {field_name} requires an integer or boolean")
        })?;
        i32::try_from(value)
            .map(|value| value.to_string())
            .map_err(|_| format!("packaged data field {field_name} is outside i32 range"))
    }

    fn unsigned_literal(
        value: &serde_json::Value,
        field_name: &str,
        max: u64,
    ) -> Result<String, String> {
        let value = value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
            .ok_or_else(|| {
                format!("packaged data field {field_name} requires an unsigned integer")
            })?;
        (value <= max)
            .then(|| value.to_string())
            .ok_or_else(|| format!("packaged data field {field_name} is outside unsigned range"))
    }

    fn float_literal(
        value: &serde_json::Value,
        field_name: &str,
        suffix: &str,
    ) -> Result<String, String> {
        let value = value
            .as_f64()
            .ok_or_else(|| format!("packaged data field {field_name} requires a number"))?;
        if !value.is_finite() {
            return Err(format!("packaged data field {field_name} must be finite"));
        }
        Ok(format!("{value:.17}{suffix}"))
    }

    let runtime_path = field
        .runtime_path
        .clone()
        .unwrap_or_else(|| field.name.replace("__", "."));
    let scalar_hash = crate::hash_global_path(&runtime_path);
    let collection_hash = field
        .collection_path
        .as_deref()
        .map(crate::hash_global_path)
        .unwrap_or_else(|| crate::hash_global_path(&runtime_path));
    let field_hash = field
        .collection_field
        .as_deref()
        .map(crate::hash_global_path)
        .unwrap_or(0);
    let is_array = field.collection_path.is_some() || field.array_count > 1;
    match field.field_type.as_str() {
        "u8" => {
            let values = values_for_field(field)?;
            let initializer = if values.is_empty() {
                "0".to_string()
            } else {
                values
                    .iter()
                    .map(|value| unsigned_literal(value, &field.name, u64::from(u8::MAX)))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            };
            source.push_str(&format!(
                "STASIS_EXPORT uint8_t {}[{}] = {{{initializer}}};\n",
                field.name,
                if is_array { field.array_count } else { 1 },
            ));
            register_lines.push(format!(
                "stasis_jit_register_global_u8_array({collection_hash}, {field_hash}, {name}, {len});",
                name = field.name,
                len = if is_array { field.array_count } else { 1 }
            ));
        }
        "u16" => {
            let values = values_for_field(field)?;
            let initializer = if values.is_empty() {
                "0".to_string()
            } else {
                values
                    .iter()
                    .map(|value| unsigned_literal(value, &field.name, u64::from(u16::MAX)))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            };
            source.push_str(&format!(
                "STASIS_EXPORT uint16_t {}[{}] = {{{initializer}}};\n",
                field.name,
                if is_array { field.array_count } else { 1 },
            ));
            register_lines.push(format!(
                "stasis_jit_register_global_u16_array({collection_hash}, {field_hash}, {name}, {len});",
                name = field.name,
                len = if is_array { field.array_count } else { 1 }
            ));
        }
        "u32" => {
            let values = values_for_field(field)?;
            let initializer = if values.is_empty() {
                "0".to_string()
            } else {
                values
                    .iter()
                    .map(|value| unsigned_literal(value, &field.name, u64::from(u32::MAX)))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            };
            if is_array {
                source.push_str(&format!(
                    "STASIS_EXPORT uint32_t {}[{}] = {{{initializer}}};\n",
                    field.name, field.array_count,
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_array({collection_hash}, {field_hash}, (int32_t*){name}, {len});",
                    name = field.name,
                    len = field.array_count
                ));
            } else {
                source.push_str(&format!(
                    "STASIS_EXPORT uint32_t {} = {initializer};\n",
                    field.name
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_ptr({scalar_hash}, (int32_t*)&{name});",
                    name = field.name
                ));
            }
        }
        "bool" | "i32" => {
            let values = values_for_field(field)?;
            if is_array {
                let initializer = if values.is_empty() {
                    "0".to_string()
                } else {
                    values
                        .iter()
                        .map(|value| i32_literal(value, &field.name))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ")
                };
                source.push_str(&format!(
                    "STASIS_EXPORT int32_t {}[{}] = {{{initializer}}};\n",
                    field.name, field.array_count,
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_array({collection_hash}, {field_hash}, {name}, {len});",
                    name = field.name,
                    len = field.array_count
                ));
            } else {
                let initializer = values
                    .first()
                    .map(|value| i32_literal(value, &field.name))
                    .transpose()?
                    .unwrap_or_else(|| "0".to_string());
                source.push_str(&format!(
                    "STASIS_EXPORT int32_t {} = {initializer};\n",
                    field.name
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_ptr({scalar_hash}, &{name});",
                    name = field.name
                ));
            }
        }
        "f32" => {
            let values = values_for_field(field)?;
            if is_array {
                let initializer = if values.is_empty() {
                    "0.0f".to_string()
                } else {
                    values
                        .iter()
                        .map(|value| float_literal(value, &field.name, "f"))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ")
                };
                source.push_str(&format!(
                    "STASIS_EXPORT float {}[{}] = {{{initializer}}};\n",
                    field.name, field.array_count,
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_f32_array({collection_hash}, {field_hash}, {name}, {len});",
                    name = field.name,
                    len = field.array_count
                ));
            } else {
                let initializer = values
                    .first()
                    .map(|value| float_literal(value, &field.name, "f"))
                    .transpose()?
                    .unwrap_or_else(|| "0.0f".to_string());
                source.push_str(&format!(
                    "STASIS_EXPORT float {} = {initializer};\n",
                    field.name
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_f32_ptr({scalar_hash}, &{name});",
                    name = field.name
                ));
            }
        }
        "f64" => {
            let values = values_for_field(field)?;
            if is_array {
                let initializer = if values.is_empty() {
                    "0.0".to_string()
                } else {
                    values
                        .iter()
                        .map(|value| float_literal(value, &field.name, ""))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ")
                };
                source.push_str(&format!(
                    "STASIS_EXPORT double {}[{}] = {{{initializer}}};\n",
                    field.name, field.array_count,
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_f64_array({collection_hash}, {field_hash}, {name}, {len});",
                    name = field.name,
                    len = field.array_count
                ));
            } else {
                let initializer = values
                    .first()
                    .map(|value| float_literal(value, &field.name, ""))
                    .transpose()?
                    .unwrap_or_else(|| "0.0".to_string());
                source.push_str(&format!(
                    "STASIS_EXPORT double {} = {initializer};\n",
                    field.name
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_f64_ptr({scalar_hash}, &{name});",
                    name = field.name
                ));
            }
        }
        "string" => {
            let len = field.size.max(1);
            let header_bytes = field.size.saturating_sub(field.array_count);
            let payload_len = field.array_count.max(1);
            source.push_str(&format!(
                "STASIS_EXPORT uint8_t {}[{}] = {{0}};\n",
                field.name, len
            ));
            if let Some(value) = field.initial_value.as_ref() {
                let text = value.as_str().ok_or_else(|| {
                    format!("packaged data field {} requires a string", field.name)
                })?;
                let bytes = text.as_bytes();
                if bytes.len() > payload_len {
                    return Err(format!(
                        "packaged data field {} exceeds string capacity {}",
                        field.name, payload_len
                    ));
                }
                if header_bytes >= 8 {
                    register_lines.push(format!(
                        "*((int32_t*){name}) = {len};",
                        name = field.name,
                        len = bytes.len()
                    ));
                }
                if header_bytes >= 12 {
                    register_lines.push(format!(
                        "*((int32_t*)({name} + 8)) = {len};",
                        name = field.name,
                        len = text.chars().count()
                    ));
                }
                for (index, byte) in bytes.iter().enumerate() {
                    register_lines.push(format!(
                        "{name}[{offset}] = {byte};",
                        name = field.name,
                        offset = header_bytes + index
                    ));
                }
            }
            if header_bytes >= 8 {
                let max_length_hash =
                    crate::hash_global_path(&format!("{}.max_length", runtime_path));
                register_lines.push(format!(
                    "*((int32_t*)({name} + 4)) = {payload_len};",
                    name = field.name
                ));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_ptr({max_length_hash}, (int32_t*)({name} + 4));",
                    name = field.name
                ));
            }
            if header_bytes >= 8 {
                let length_hash = crate::hash_global_path(&format!("{}.length", runtime_path));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_ptr({length_hash}, (int32_t*){name});",
                    name = field.name
                ));
            }
            if header_bytes >= 12 {
                let char_length_hash =
                    crate::hash_global_path(&format!("{}.char_length", runtime_path));
                register_lines.push(format!(
                    "stasis_jit_register_global_i32_ptr({char_length_hash}, (int32_t*)({name} + 8));",
                    name = field.name
                ));
            }
            register_lines.push(format!(
                "stasis_jit_register_global_u8_array({scalar_hash}, 0, {name} + {header_bytes}, {payload_len});",
                name = field.name,
                header_bytes = header_bytes,
                payload_len = payload_len
            ));
        }
        other => {
            return Err(format!(
                "unsupported packaged runtime field type {other} for {}",
                field.name
            ));
        }
    }
    Ok(())
}

fn escape_c_string_literal(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => out.push_str(&format!("\\x{:02X}", c as u32)),
        }
    }
    out
}

fn replay_snapshot_c_storage_type(storage_type: &str) -> Option<&'static str> {
    match storage_type {
        "bool" | "i32" => Some("int32_t"),
        "u8" => Some("uint8_t"),
        "u16" => Some("uint16_t"),
        "u32" => Some("uint32_t"),
        "f32" => Some("float"),
        "f64" => Some("double"),
        _ => None,
    }
}

fn replay_snapshot_element_width(storage_type: &str) -> Option<u8> {
    match storage_type {
        "bool" | "u8" => Some(1),
        "u16" => Some(2),
        "i32" | "u32" | "f32" => Some(4),
        "f64" => Some(8),
        _ => None,
    }
}

fn replay_snapshot_storage_symbol(entry: &ProgramReplayStateEntry) -> String {
    let kind = if entry.kind == "scalar" {
        AotStorageSymbolKind::Scalar
    } else if entry.kind == "collection" {
        AotStorageSymbolKind::Array
    } else {
        // Validation reports the unsupported kind before symbol generation. Keep this
        // fallback deterministic so malformed descriptors cannot influence symbol spelling.
        AotStorageSymbolKind::Scalar
    };
    aot_storage_symbol(kind, &entry.path, &entry.field)
}

pub fn replay_snapshot_bridge_can_emit(snapshot: &ProgramReplayStateSnapshot) -> bool {
    if snapshot.schema != "stasis.replay_state_snapshot.v2"
        || snapshot.abi_version != 2
        || !matches!(
            snapshot.support.as_str(),
            "descriptor_only" | "canonical_bytes"
        )
        || snapshot.byte_order != "little_endian"
        || snapshot.hash_scope != "simulation_after_tick"
        || snapshot.size_operation != "stasis_replay_state_snapshot_size"
        || snapshot.write_operation != "stasis_replay_state_snapshot_write"
        || snapshot.restore_operation != "stasis_replay_state_snapshot_restore"
        || !snapshot.unsupported_paths.is_empty()
        || snapshot.required_bytes > i64::from(i32::MAX) as u64
    {
        return false;
    }
    let mut expected_offset = 0u64;
    let mut symbols = BTreeSet::new();
    let mut saw_collection = false;
    let mut previous_scalar_path: Option<&str> = None;
    let mut previous_collection_key: Option<(&str, &str)> = None;
    for entry in &snapshot.entries {
        if !matches!(entry.kind.as_str(), "scalar" | "collection")
            || (entry.kind == "scalar" && !entry.field.is_empty())
            || (entry.kind == "scalar" && entry.element_count != 1)
            || replay_snapshot_element_width(&entry.storage_type) != Some(entry.element_bytes)
            || entry.offset != expected_offset
        {
            return false;
        }
        if entry.kind == "scalar" {
            if saw_collection
                || previous_scalar_path.is_some_and(|previous| previous >= entry.path.as_str())
            {
                return false;
            }
            previous_scalar_path = Some(&entry.path);
        } else {
            saw_collection = true;
            let key = (entry.path.as_str(), entry.field.as_str());
            if previous_collection_key.is_some_and(|previous| previous >= key) {
                return false;
            }
            previous_collection_key = Some(key);
        }
        let Some(entry_bytes) = entry
            .element_count
            .checked_mul(u64::from(entry.element_bytes))
        else {
            return false;
        };
        let Some(next_offset) = expected_offset.checked_add(entry_bytes) else {
            return false;
        };
        expected_offset = next_offset;
        if entry_bytes > 0 && !symbols.insert(replay_snapshot_storage_symbol(entry)) {
            return false;
        }
    }
    expected_offset == snapshot.required_bytes
}

pub(crate) fn append_replay_state_snapshot_bridge_source(
    source: &mut String,
    snapshot: &ProgramReplayStateSnapshot,
) -> Result<(), String> {
    if !replay_snapshot_bridge_can_emit(snapshot) {
        return Err(
            "replay state snapshot descriptor is not canonical for the native bridge".to_string(),
        );
    }
    if snapshot.schema != "stasis.replay_state_snapshot.v2" {
        return Err(format!(
            "unsupported replay state snapshot schema '{}'",
            snapshot.schema
        ));
    }
    if snapshot.abi_version != 2 {
        return Err(format!(
            "unsupported replay state snapshot ABI version {}",
            snapshot.abi_version
        ));
    }
    if snapshot.byte_order != "little_endian" {
        return Err(format!(
            "replay state snapshot requires little-endian bytes, found '{}'",
            snapshot.byte_order
        ));
    }
    if !snapshot.unsupported_paths.is_empty() {
        return Err(format!(
            "replay state snapshot has unsupported paths: {}",
            snapshot.unsupported_paths.join(", ")
        ));
    }
    let required_bytes = i32::try_from(snapshot.required_bytes).map_err(|_| {
        format!(
            "replay state snapshot requires {} bytes, exceeding the native ABI limit",
            snapshot.required_bytes
        )
    })?;

    let mut expected_offset = 0u64;
    let mut declared_symbols = BTreeSet::new();
    for entry in &snapshot.entries {
        if !matches!(entry.kind.as_str(), "scalar" | "collection") {
            return Err(format!(
                "replay state snapshot entry '{}.{}' has unsupported kind '{}'",
                entry.path, entry.field, entry.kind
            ));
        }
        if entry.kind == "scalar" && !entry.field.is_empty() {
            return Err(format!(
                "scalar replay state snapshot entry '{}' must not have a field",
                entry.path
            ));
        }
        let Some(c_type) = replay_snapshot_c_storage_type(&entry.storage_type) else {
            return Err(format!(
                "replay state snapshot entry '{}.{}' has unsupported storage type '{}'",
                entry.path, entry.field, entry.storage_type
            ));
        };
        let Some(expected_width) = replay_snapshot_element_width(&entry.storage_type) else {
            return Err(format!(
                "replay state snapshot entry '{}.{}' has no canonical width",
                entry.path, entry.field
            ));
        };
        if entry.element_bytes != expected_width {
            return Err(format!(
                "replay state snapshot entry '{}.{}' declares {} bytes for type '{}', expected {}",
                entry.path, entry.field, entry.element_bytes, entry.storage_type, expected_width
            ));
        }
        if entry.offset != expected_offset {
            return Err(format!(
                "replay state snapshot entry '{}.{}' starts at {}, expected {}",
                entry.path, entry.field, entry.offset, expected_offset
            ));
        }
        let entry_bytes = entry
            .element_count
            .checked_mul(u64::from(entry.element_bytes))
            .ok_or_else(|| {
                format!(
                    "replay state snapshot entry '{}.{}' byte count overflow",
                    entry.path, entry.field
                )
            })?;
        expected_offset = expected_offset
            .checked_add(entry_bytes)
            .ok_or_else(|| "replay state snapshot byte count overflow".to_string())?;
        if entry_bytes == 0 {
            continue;
        }
        let symbol = replay_snapshot_storage_symbol(entry);
        if !declared_symbols.insert(symbol.clone()) {
            return Err(format!(
                "replay state snapshot contains duplicate storage symbol '{symbol}'"
            ));
        }
        if entry.kind == "scalar" {
            source.push_str(&format!("STASIS_EXPORT extern {c_type} {symbol};\n"));
        } else {
            source.push_str(&format!("STASIS_EXPORT extern {c_type} {symbol}[];\n"));
        }
    }
    if expected_offset != snapshot.required_bytes {
        return Err(format!(
            "replay state snapshot requires {} bytes but entries cover {}",
            snapshot.required_bytes, expected_offset
        ));
    }

    if required_bytes > 0
        && snapshot.entries.iter().any(|entry| {
            entry.element_count > 0 && matches!(entry.storage_type.as_str(), "i32" | "f32" | "f64")
        })
    {
        source.push_str(
            r#"static void stasis_copy_bytes(void *destination, const void *source, uint32_t byte_count) {
    uint8_t *destination_bytes = (uint8_t *)destination;
    const uint8_t *source_bytes = (const uint8_t *)source;
    uint32_t index;
    for (index = 0; index < byte_count; ++index) destination_bytes[index] = source_bytes[index];
}
"#,
        );
    }
    source.push_str("STASIS_EXPORT int32_t stasis_replay_state_snapshot_size(void) {\n");
    source.push_str(&format!("    return {required_bytes};\n"));
    source.push_str("}\n");
    source.push_str(
        "STASIS_EXPORT int32_t stasis_replay_state_snapshot_write(uint8_t *out, int32_t capacity) {\n",
    );
    if required_bytes == 0 {
        source.push_str("    (void)out;\n    (void)capacity;\n    return 0;\n}\n");
    } else {
        source.push_str(&format!(
            "    const int32_t required = {required_bytes};\n    if (out == (uint8_t *)0 || capacity < required) return -required;\n    uint32_t bits32 = 0;\n    uint64_t bits64 = 0;\n"
        ));

        for entry in &snapshot.entries {
            if entry.element_count == 0 {
                continue;
            }
            let symbol = replay_snapshot_storage_symbol(entry);
            for index in 0..entry.element_count {
                let element_offset = entry
                    .offset
                    .checked_add(index * u64::from(entry.element_bytes))
                    .ok_or_else(|| "replay state snapshot element offset overflow".to_string())?;
                let element_offset = i32::try_from(element_offset).map_err(|_| {
                    "replay state snapshot element offset exceeds the native ABI limit".to_string()
                })?;
                let expression = if entry.kind == "scalar" {
                    symbol.clone()
                } else {
                    format!("{symbol}[{index}]")
                };
                match entry.storage_type.as_str() {
                    "bool" => source.push_str(&format!(
                        "    out[{element_offset}] = (uint8_t)(((int32_t)({expression}) != 0) ? 1 : 0);\n"
                    )),
                    "u8" => source.push_str(&format!(
                        "    out[{element_offset}] = (uint8_t)({expression});\n"
                    )),
                    "u16" => source.push_str(&format!(
                        "    bits32 = (uint32_t)({expression});\n    out[{element_offset}] = (uint8_t)(bits32 & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 8) & 0xffu);\n",
                        element_offset + 1
                    )),
                    "i32" | "u32" => source.push_str(&format!(
                        "    bits32 = (uint32_t)({expression});\n    out[{element_offset}] = (uint8_t)(bits32 & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 8) & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 16) & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 24) & 0xffu);\n",
                        element_offset + 1,
                        element_offset + 2,
                        element_offset + 3
                    )),
                    "f32" => source.push_str(&format!(
                        "    stasis_copy_bytes(&bits32, &({expression}), 4u);\n    out[{element_offset}] = (uint8_t)(bits32 & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 8) & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 16) & 0xffu);\n    out[{}] = (uint8_t)((bits32 >> 24) & 0xffu);\n",
                        element_offset + 1,
                        element_offset + 2,
                        element_offset + 3
                    )),
                    "f64" => source.push_str(&format!(
                        "    stasis_copy_bytes(&bits64, &({expression}), 8u);\n    out[{element_offset}] = (uint8_t)(bits64 & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 8) & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 16) & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 24) & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 32) & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 40) & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 48) & 0xffull);\n    out[{}] = (uint8_t)((bits64 >> 56) & 0xffull);\n",
                        element_offset + 1,
                        element_offset + 2,
                        element_offset + 3,
                        element_offset + 4,
                        element_offset + 5,
                        element_offset + 6,
                        element_offset + 7
                    )),
                    other => {
                        return Err(format!(
                            "replay state snapshot entry '{}.{}' has unsupported type '{other}'",
                            entry.path, entry.field
                        ));
                    }
                }
            }
        }
        source.push_str("    return required;\n}\n");
    }

    source.push_str(
        "STASIS_EXPORT int32_t stasis_replay_state_snapshot_restore(const uint8_t *input, int32_t bytes) {\n",
    );
    source.push_str(&format!(
        "    const int32_t required = {required_bytes};\n    if (bytes != required) return required == 0 ? -1 : -required;\n"
    ));
    if required_bytes == 0 {
        source.push_str("    (void)input;\n    return 0;\n}\n");
        return Ok(());
    }
    source.push_str("    if (input == (const uint8_t *)0) return -required;\n");

    // Exact-size and null checks above precede a complete, unrolled decode that overwrites
    // every represented value, so no separate preclear is needed.
    source.push_str("    uint32_t bits32 = 0;\n    uint64_t bits64 = 0;\n");
    for entry in &snapshot.entries {
        if entry.element_count == 0 {
            continue;
        }
        let symbol = replay_snapshot_storage_symbol(entry);
        for index in 0..entry.element_count {
            let element_offset = entry
                .offset
                .checked_add(index * u64::from(entry.element_bytes))
                .ok_or_else(|| "replay state snapshot element offset overflow".to_string())?;
            let element_offset = i32::try_from(element_offset).map_err(|_| {
                "replay state snapshot element offset exceeds the native ABI limit".to_string()
            })?;
            let expression = if entry.kind == "scalar" {
                symbol.clone()
            } else {
                format!("{symbol}[{index}]")
            };
            match entry.storage_type.as_str() {
                "bool" => source.push_str(&format!(
                    "    {expression} = (input[{element_offset}] != 0) ? 1 : 0;\n"
                )),
                "u8" => source.push_str(&format!(
                    "    {expression} = input[{element_offset}];\n"
                )),
                "u16" => source.push_str(&format!(
                    "    bits32 = ((uint32_t)input[{element_offset}]) | ((uint32_t)input[{}] << 8);\n    {expression} = (uint16_t)bits32;\n",
                    element_offset + 1
                )),
                "i32" | "u32" | "f32" => source.push_str(&format!(
                    "    bits32 = ((uint32_t)input[{element_offset}]) | ((uint32_t)input[{}] << 8) | ((uint32_t)input[{}] << 16) | ((uint32_t)input[{}] << 24);\n",
                    element_offset + 1,
                    element_offset + 2,
                    element_offset + 3
                )),
                "f64" => source.push_str(&format!(
                    "    bits64 = ((uint64_t)input[{element_offset}]) | ((uint64_t)input[{}] << 8) | ((uint64_t)input[{}] << 16) | ((uint64_t)input[{}] << 24) | ((uint64_t)input[{}] << 32) | ((uint64_t)input[{}] << 40) | ((uint64_t)input[{}] << 48) | ((uint64_t)input[{}] << 56);\n",
                    element_offset + 1,
                    element_offset + 2,
                    element_offset + 3,
                    element_offset + 4,
                    element_offset + 5,
                    element_offset + 6,
                    element_offset + 7
                )),
                other => {
                    return Err(format!(
                        "replay state snapshot entry '{}.{}' has unsupported type '{other}'",
                        entry.path, entry.field
                    ));
                }
            }
            match entry.storage_type.as_str() {
                "i32" | "f32" => source.push_str(&format!(
                    "    stasis_copy_bytes(&({expression}), &bits32, 4u);\n"
                )),
                "u32" => source.push_str(&format!("    {expression} = bits32;\n")),
                "f64" => source.push_str(&format!(
                    "    stasis_copy_bytes(&({expression}), &bits64, 8u);\n"
                )),
                "bool" | "u8" | "u16" => {}
                other => {
                    return Err(format!(
                        "replay state snapshot entry '{}.{}' has unsupported type '{other}'",
                        entry.path, entry.field
                    ));
                }
            }
        }
    }
    source.push_str("    return required;\n}\n");
    Ok(())
}

#[cfg(test)]
fn build_engine_bundle_runtime_bridge_source(
    target: &stasis_jit::AotTarget,
    runtime_fields: &[PackagedRuntimeField],
    function_symbols: &[String],
    function_aliases: &[PackagedFunctionAlias],
    render_alias: Option<&PackagedRenderAlias>,
    string_literals: &[EngineBundleManifestStringLiteralRow],
) -> Result<String, String> {
    build_engine_bundle_runtime_bridge_source_with_snapshot(
        target,
        runtime_fields,
        function_symbols,
        function_aliases,
        render_alias,
        string_literals,
        None,
    )
}

fn build_engine_bundle_runtime_bridge_source_with_snapshot(
    target: &stasis_jit::AotTarget,
    runtime_fields: &[PackagedRuntimeField],
    function_symbols: &[String],
    function_aliases: &[PackagedFunctionAlias],
    render_alias: Option<&PackagedRenderAlias>,
    string_literals: &[EngineBundleManifestStringLiteralRow],
    replay_state_snapshot: Option<&ProgramReplayStateSnapshot>,
) -> Result<String, String> {
    let host_i32_hash = crate::hash_global_path("host_i32");
    let host_f32_hash = crate::hash_global_path("host_f32");
    let gfx_cmd_i32_hash = crate::hash_global_path("gfx_cmd_i32");
    let gfx_cmd_f32_hash = crate::hash_global_path("gfx_cmd_f32");
    let gfx_cmd_u8_hash = crate::hash_global_path("gfx_cmd_u8");
    let host_req_seq_hash = crate::hash_global_path("host_req_seq");
    let host_req_flags_hash = crate::hash_global_path("host_req_flags");
    let host_req_window_w_px_hash = crate::hash_global_path("host_req_window_w_px");
    let host_req_window_h_px_hash = crate::hash_global_path("host_req_window_h_px");

    let mut source = String::new();
    source.push_str(
        "#if defined(_WIN32)\n\
typedef signed int int32_t;\n\
typedef signed long long int64_t;\n\
typedef unsigned char uint8_t;\n\
typedef unsigned short uint16_t;\n\
typedef unsigned int uint32_t;\n\
typedef unsigned long long uint64_t;\n\
typedef unsigned long long uintptr_t;\n\
#else\n\
#include <stdint.h>\n\
#endif\n\
",
    );
    source.push_str(
        "#if defined(_WIN32)\n\
#define STASIS_EXPORT __declspec(dllexport)\n\
#else\n\
#define STASIS_EXPORT __attribute__((visibility(\"default\")))\n\
#endif\n",
    );
    if let Some(snapshot) =
        replay_state_snapshot.filter(|snapshot| replay_snapshot_bridge_can_emit(snapshot))
    {
        append_replay_state_snapshot_bridge_source(&mut source, snapshot)?;
    }
    source.push_str(
        "void stasis_jit_register_global_i32_ptr(int32_t path_hash, int32_t* ptr);\n\
void stasis_jit_register_global_f32_ptr(int32_t path_hash, float* ptr);\n\
void stasis_jit_register_global_f64_ptr(int32_t path_hash, double* ptr);\n\
void stasis_jit_register_global_i32_array(int32_t collection_hash, int32_t field_hash, int32_t* ptr, int32_t len);\n\
void stasis_jit_register_global_f32_array(int32_t collection_hash, int32_t field_hash, float* ptr, int32_t len);\n\
void stasis_jit_register_global_f64_array(int32_t collection_hash, int32_t field_hash, double* ptr, int32_t len);\n\
void stasis_jit_register_global_u8_array(int32_t collection_hash, int32_t field_hash, uint8_t* ptr, int32_t len);\n\
void stasis_jit_register_global_u16_array(int32_t collection_hash, int32_t field_hash, uint16_t* ptr, int32_t len);\n\
void stasis_jit_clear_string_literal_table(void);\n\
void stasis_jit_upsert_string_literal(int32_t id, const char* value);\n",
    );

    for symbol in function_symbols {
        source.push_str(&format!("void {}(void);\n", symbol));
    }
    for literal in string_literals {
        source.push_str(&format!(
            "static const char stasis_literal_{}[] = \"{}\";\n",
            literal.id.unsigned_abs(),
            escape_c_string_literal(&literal.value)
        ));
    }

    source.push_str(
        "STASIS_EXPORT int32_t host_i32[768] = {0};\n\
STASIS_EXPORT float host_f32[64] = {0};\n\
STASIS_EXPORT int32_t gfx_cmd_i32[67888] = {0};\n\
STASIS_EXPORT float gfx_cmd_f32[146564] = {0};\n\
STASIS_EXPORT uint8_t gfx_cmd_u8[65536] = {0};\n\
STASIS_EXPORT int32_t host_req_seq = 0;\n\
STASIS_EXPORT int32_t host_req_flags = 0;\n\
STASIS_EXPORT int32_t host_req_window_w_px = 0;\n\
STASIS_EXPORT int32_t host_req_window_h_px = 0;\n",
    );

    // AOT objects use kind-qualified storage names. Keep the legacy bridge
    // exports as aliases because the desktop runner discovers these buffers
    // by their original names.
    let bridge_storage_aliases = [
        (
            aot_storage_symbol(AotStorageSymbolKind::Array, "host_i32", ""),
            "host_i32",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Array, "host_f32", ""),
            "host_f32",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_i32", ""),
            "gfx_cmd_i32",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_f32", ""),
            "gfx_cmd_f32",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_u8", ""),
            "gfx_cmd_u8",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_seq", ""),
            "host_req_seq",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_flags", ""),
            "host_req_flags",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_window_w_px", ""),
            "host_req_window_w_px",
        ),
        (
            aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_window_h_px", ""),
            "host_req_window_h_px",
        ),
    ];
    let use_msvc_aliases = matches!(target, stasis_jit::AotTarget::Native) && cfg!(windows);
    let use_darwin_aliases = matches!(target, stasis_jit::AotTarget::IosArm64)
        || (matches!(target, stasis_jit::AotTarget::Native) && cfg!(target_os = "macos"));
    if use_msvc_aliases {
        for (qualified, legacy) in &bridge_storage_aliases {
            source.push_str(&format!(
                "#pragma comment(linker, \"/alternatename:{qualified}={legacy}\")\n"
            ));
        }
    } else if use_darwin_aliases {
        for (qualified, legacy) in &bridge_storage_aliases {
            source.push_str(&format!(
                "__asm__(\".globl _{qualified}\\n_{qualified} = _{legacy}\");\n"
            ));
        }
    } else {
        for (qualified, legacy) in &bridge_storage_aliases {
            source.push_str(&format!(
                "extern __typeof__({legacy}) {qualified} __attribute__((alias(\"{legacy}\")));\n"
            ));
        }
    }

    let mut register_lines = vec![
        format!(
            "stasis_jit_register_global_i32_array({host_i32_hash}, 0, host_i32, 768);"
        ),
        format!(
            "stasis_jit_register_global_f32_array({host_f32_hash}, 0, host_f32, 64);"
        ),
        format!(
            "stasis_jit_register_global_i32_array({gfx_cmd_i32_hash}, 0, gfx_cmd_i32, 67888);"
        ),
        format!(
            "stasis_jit_register_global_f32_array({gfx_cmd_f32_hash}, 0, gfx_cmd_f32, 146564);"
        ),
        format!(
            "stasis_jit_register_global_u8_array({gfx_cmd_u8_hash}, 0, gfx_cmd_u8, 65536);"
        ),
        format!(
            "stasis_jit_register_global_i32_ptr({host_req_seq_hash}, &host_req_seq);"
        ),
        format!(
            "stasis_jit_register_global_i32_ptr({host_req_flags_hash}, &host_req_flags);"
        ),
        format!(
            "stasis_jit_register_global_i32_ptr({host_req_window_w_px_hash}, &host_req_window_w_px);"
        ),
        format!(
            "stasis_jit_register_global_i32_ptr({host_req_window_h_px_hash}, &host_req_window_h_px);"
        ),
    ];

    for field in runtime_fields {
        append_runtime_bridge_field_source(&mut source, &mut register_lines, field)?;
    }
    register_lines.push("stasis_jit_clear_string_literal_table();".to_string());
    for literal in string_literals {
        register_lines.push(format!(
            "stasis_jit_upsert_string_literal({id}, stasis_literal_{name});",
            id = literal.id,
            name = literal.id.unsigned_abs()
        ));
    }

    for alias in function_aliases {
        if alias.returns_i32 {
            source.push_str(&format!(
                "STASIS_EXPORT int32_t {alias}(void) {{ return ((int32_t (*)(void)){target})(); }}\n",
                alias = alias.alias,
                target = alias.target_symbol
            ));
        } else {
            source.push_str(&format!(
                "STASIS_EXPORT void {alias}(void) {{ ((void (*)(void)){target})(); }}\n",
                alias = alias.alias,
                target = alias.target_symbol
            ));
        }
    }
    if let Some(render) = render_alias {
        match (
            render.returns_i32,
            render.reset_symbol.as_deref(),
            render.finish_symbol.as_deref(),
        ) {
            (true, Some(reset), Some(finish)) => source.push_str(&format!(
                "STASIS_EXPORT int32_t render(void) {{ ((void (*)(void)){reset})(); int32_t result = ((int32_t (*)(void)){target})(); return ((int32_t (*)(int32_t)){finish})(result); }}\n",
                target = render.target_symbol,
            )),
            (false, Some(reset), Some(finish)) => source.push_str(&format!(
                "STASIS_EXPORT int32_t render(void) {{ ((void (*)(void)){reset})(); ((void (*)(void)){target})(); return ((int32_t (*)(int32_t)){finish})(0); }}\n",
                target = render.target_symbol,
            )),
            (true, None, None) => source.push_str(&format!(
                "STASIS_EXPORT int32_t render(void) {{ return ((int32_t (*)(void)){target})(); }}\n",
                target = render.target_symbol,
            )),
            (false, None, None) => source.push_str(&format!(
                "STASIS_EXPORT int32_t render(void) {{ ((void (*)(void)){target})(); return 0; }}\n",
                target = render.target_symbol,
            )),
            _ => {
                return Err(
                    "packaged render construction lifecycle requires both reset and finish helpers"
                        .to_string(),
                )
            }
        }
    }
    // Keep the Android ABI surface fixed while the host shell/input event mapping lands separately.
    if target.is_android() {
        source.push_str(
            "STASIS_EXPORT void stasis_init(int width, int height) {\n\
    host_i32[12] = width;\n\
    host_i32[13] = height;\n\
    host_i32[14] = 4;\n\
    host_i32[22] = width;\n\
    host_i32[23] = height;\n\
    host_i32[24] = width;\n\
    host_i32[25] = height;\n\
    host_i32[30] = 1;\n\
    host_i32[31] = 1;\n\
    host_f32[48] = 1.0f;\n\
    host_f32[49] = 1.0f;\n\
    host_f32[50] = (float)width;\n\
    host_f32[51] = (float)height;\n\
    host_f32[52] = 0.0f;\n\
    host_f32[53] = 0.0f;\n\
    host_f32[54] = (float)width;\n\
    host_f32[55] = (float)height;\n\
    host_f32[56] = (float)width;\n\
    host_f32[57] = (float)height;\n\
    host_req_window_w_px = width;\n\
    host_req_window_h_px = height;\n\
    main();\n\
}\n\
STASIS_EXPORT void stasis_tick(float dt) {\n\
    (void)dt;\n\
    host_i32[10] = host_i32[10] + 1;\n\
    tick();\n\
}\n\
STASIS_EXPORT void stasis_render(void) {\n\
    render();\n\
}\n\
STASIS_EXPORT void stasis_on_input(int type, int a, int b) {\n\
    (void)type;\n\
    (void)a;\n\
    (void)b;\n\
}\n",
        );
    }
    source.push_str("STASIS_EXPORT void stasis_aot_bind_runtime_globals(void) {\n");
    for line in register_lines {
        source.push_str("    ");
        source.push_str(&line);
        source.push('\n');
    }
    source.push_str("}\n");
    Ok(source)
}

fn emit_engine_bundle_runtime_bridge_object(
    backend: &IncrementalCompilerBackend,
    runtime_fields: &[PackagedRuntimeField],
    function_symbols: &[String],
    function_aliases: &[PackagedFunctionAlias],
    render_alias: Option<&PackagedRenderAlias>,
    string_literals: &[EngineBundleManifestStringLiteralRow],
    host_exports: Option<&stasis_compiler::host_exports::HostExports>,
) -> Result<PathBuf, String> {
    let source_path = backend
        .aot_artifact_root
        .join("engine_bundle_runtime_bridge.c");
    let object_path = backend.aot_artifact_root.join(format!(
        "engine_bundle_runtime_bridge.{}",
        runtime_bridge_object_extension(&backend.aot_compile_config.target)
    ));
    let replay_compatibility = backend
        .last_program_snapshot
        .as_ref()
        .map(ProgramSnapshot::replay_compatibility);
    let function_symbols: Vec<String> = function_symbols
        .iter()
        .filter(|symbol| {
            host_exports.is_none_or(|exports| {
                !exports
                    .functions
                    .iter()
                    .any(|record| &record.target_symbol == *symbol)
            })
        })
        .cloned()
        .collect();
    let mut source = build_engine_bundle_runtime_bridge_source_with_snapshot(
        &backend.aot_compile_config.target,
        runtime_fields,
        &function_symbols,
        function_aliases,
        render_alias,
        string_literals,
        replay_compatibility
            .as_ref()
            .map(|compatibility| &compatibility.state_snapshot)
            .filter(|snapshot| replay_snapshot_bridge_can_emit(snapshot)),
    )?;
    if let Some(exports) = host_exports {
        source.push_str(&exports.c_wrappers()?);
    }
    std::fs::write(&source_path, source).map_err(|error| {
        format!(
            "failed to write engine bundle runtime bridge source {}: {error}",
            source_path.display()
        )
    })?;

    let compiler = std::env::var_os("CC")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_runtime_bridge_compiler(&backend.aot_compile_config.target));
    let compiler_name = compiler
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_msvc_style = matches!(
        backend.aot_compile_config.target,
        stasis_jit::AotTarget::Native
    ) && cfg!(windows)
        && (compiler_name == "clang-cl"
            || compiler_name == "clang-cl.exe"
            || compiler_name == "cl"
            || compiler_name == "cl.exe");
    let mut command = std::process::Command::new(&compiler);
    if is_msvc_style {
        command
            .arg("/nologo")
            .arg("/c")
            .arg("/O2")
            .arg("/TC")
            .arg("/Zl")
            .arg("/GS-")
            .arg("/X")
            .arg(format!("/Fo{}", object_path.display()))
            .arg(&source_path);
    } else {
        command
            .arg("-c")
            .arg("-O2")
            .arg("-x")
            .arg("c")
            .arg("-o")
            .arg(&object_path);
        if !cfg!(windows) || backend.aot_compile_config.target.is_android() {
            command.arg("-fPIC");
        }
        if let Some(target) = backend.aot_compile_config.target.clang_target() {
            command.arg(format!("--target={target}"));
        }
        command.arg(&source_path);
    }
    let output = command.output().map_err(|error| {
        format!(
            "failed to spawn C compiler {:?} for runtime bridge object: {error}",
            compiler
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "failed to build engine bundle runtime bridge object\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !object_path.exists() {
        return Err(format!(
            "runtime bridge compile succeeded but did not produce {}",
            object_path.display()
        ));
    }
    Ok(object_path)
}

fn resolve_engine_bundle_symbol(
    manifest: &EngineBundleManifest,
    name: &str,
) -> Result<String, String> {
    manifest
        .functions
        .iter()
        .find(|row| row.name == name && (name != "tick" || row.parameter_count == 0))
        .map(|row| row.symbol.clone())
        .ok_or_else(|| format!("engine bundle manifest is missing required symbol {name}"))
}

fn cmake_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

static MONOLITH_CMAKE_INSTANCE: AtomicU64 = AtomicU64::new(0);

struct MonolithCmakeBuildDir {
    path: PathBuf,
}

impl Drop for MonolithCmakeBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn create_monolith_cmake_build_dir(
    aot_root: &Path,
    output_exe: &Path,
) -> Result<MonolithCmakeBuildDir, String> {
    let identity = format!(
        "{}|{}",
        stable_absolute_path(aot_root).display(),
        stable_absolute_path(output_exe).display()
    );
    let identity_hash = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let parent = std::env::temp_dir().join("stasis-monolith-cmake");
    std::fs::create_dir_all(&parent).map_err(|error| {
        format!(
            "failed to create monolith CMake temp directory {}: {error}",
            parent.display()
        )
    })?;
    for _ in 0..32 {
        let instance = MONOLITH_CMAKE_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            "{}-{:016x}-{}",
            std::process::id(),
            instance,
            &identity_hash[..12]
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(MonolithCmakeBuildDir { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "failed to create isolated monolith CMake build directory {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err("could not allocate a unique isolated monolith CMake build directory".to_string())
}

#[cfg(windows)]
fn resolve_vcvars64() -> Result<PathBuf, String> {
    let mut installation_roots = Vec::new();
    let mut vswhere_paths = vec![
        PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"),
        PathBuf::from(r"C:\Program Files\Microsoft Visual Studio\Installer\vswhere.exe"),
    ];
    if let Ok(output) = std::process::Command::new("vswhere.exe")
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
    {
        if output.status.success() {
            installation_roots.extend(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from),
            );
        }
    }
    for path in vswhere_paths.drain(..) {
        if !path.is_file() {
            continue;
        }
        if let Ok(output) = std::process::Command::new(&path)
            .args([
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ])
            .output()
        {
            if output.status.success() {
                installation_roots.extend(
                    String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(PathBuf::from),
                );
            }
        }
    }
    for root in [
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools",
        r"C:\Program Files\Microsoft Visual Studio\2022\BuildTools",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\Community",
        r"C:\Program Files\Microsoft Visual Studio\2022\Community",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\Professional",
        r"C:\Program Files\Microsoft Visual Studio\2022\Professional",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\Enterprise",
        r"C:\Program Files\Microsoft Visual Studio\2022\Enterprise",
    ] {
        installation_roots.push(PathBuf::from(root));
    }
    for root in installation_roots {
        let candidate = root.join(r"VC\Auxiliary\Build\vcvars64.bat");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("Visual Studio vcvars64.bat was not found. Install the C++ desktop workload (including MSVC x64/x86 build tools) or add vswhere.exe to PATH".to_string())
}

#[cfg(windows)]
fn run_monolith_cmake(arguments: &[String]) -> Result<std::process::Output, String> {
    let vcvars = resolve_vcvars64()?;
    let quoted_arguments = arguments
        .iter()
        .map(|argument| format!("\"{}\"", argument.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("system clock failed while staging CMake: {error}"))?
        .as_nanos();
    let script_path = std::env::temp_dir().join(format!(
        "stasis_monolith_cmake_{}_{}.cmd",
        std::process::id(),
        stamp
    ));
    let script = format!(
        "@echo off\r\ncall \"{}\" >nul\r\nif errorlevel 1 exit /b %errorlevel%\r\ncmake {quoted_arguments}\r\n",
        vcvars.display()
    );
    std::fs::write(&script_path, script)
        .map_err(|error| format!("failed to stage {}: {error}", script_path.display()))?;
    let output = std::process::Command::new("cmd.exe")
        .arg("/d")
        .arg("/c")
        .arg(&script_path)
        .output()
        .map_err(|error| format!("failed to launch CMake in the MSVC environment: {error}"));
    let _ = std::fs::remove_file(&script_path);
    output
}

#[cfg(not(windows))]
fn run_monolith_cmake(arguments: &[String]) -> Result<std::process::Output, String> {
    std::process::Command::new("cmake")
        .args(arguments)
        .output()
        .map_err(|error| format!("failed to launch CMake: {error}"))
}

fn monolith_configure_arguments(
    runtime_root: &Path,
    build_dir: &Path,
    aot_root: &Path,
    shell_source: &Path,
    output_dir: &Path,
    output_name: &str,
    desktop_network: Option<&DesktopNetworkLink>,
) -> Vec<String> {
    let mut arguments = vec![
        "-S".to_string(),
        cmake_path(runtime_root),
        "-B".to_string(),
        cmake_path(build_dir),
        "-DSTASIS_BUILD_MONOLITH=ON".to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        format!("-DSTASIS_MONOLITH_AOT_DIR={}", cmake_path(aot_root)),
        format!("-DSTASIS_MONOLITH_MAIN_SOURCE={}", cmake_path(shell_source)),
        format!("-DSTASIS_MONOLITH_OUTPUT_DIR={}", cmake_path(output_dir)),
        format!("-DSTASIS_MONOLITH_OUTPUT_NAME={output_name}"),
    ];
    if let Some(network) = desktop_network {
        arguments.push(format!(
            "-DSTASIS_MONOLITH_NETWORK_LIBRARY={}",
            cmake_path(&network.library)
        ));
        arguments.push(format!(
            "-DSTASIS_MONOLITH_NETWORK_INCLUDE_DIR={}",
            cmake_path(&network.include_dir)
        ));
        arguments.push(format!(
            "-DSTASIS_MONOLITH_NETWORK_MODE={}",
            network.mode.cmake_value()
        ));
    }
    arguments
}

fn package_engine_bundle_monolithic_desktop(
    backend: &IncrementalCompilerBackend,
    bundle: &AotEngineBundle,
    output_exe: &Path,
    project_dir: &Path,
    desktop_network: Option<&DesktopNetworkLink>,
) -> Result<SelfHostedAotCliSummary, String> {
    let repo_root = self_host_repo_root()?;
    let aot_root = backend.aot_artifact_root.join("desktop_monolith");
    std::fs::create_dir_all(&aot_root).map_err(|error| {
        format!(
            "failed to create desktop monolith directory {}: {error}",
            aot_root.display()
        )
    })?;
    let manifest_text = std::fs::read_to_string(&bundle.manifest_path).map_err(|error| {
        format!(
            "failed to read AOT engine manifest {}: {error}",
            bundle.manifest_path.display()
        )
    })?;
    let manifest_json: serde_json::Value = serde_json::from_str(&manifest_text)
        .map_err(|error| format!("failed to parse AOT engine manifest: {error}"))?;
    let state_layout = backend
        .last_program_snapshot
        .as_ref()
        .map(ProgramSnapshot::state_layout)
        .ok_or_else(|| "AOT program snapshot missing during monolithic packaging".to_string())?;
    let replay_state_snapshot = backend
        .last_program_snapshot
        .as_ref()
        .map(ProgramSnapshot::replay_compatibility)
        .map(|compatibility| compatibility.state_snapshot)
        .filter(|snapshot| replay_snapshot_bridge_can_emit(snapshot));
    let replay_asset_manifest_root = aot_root.join("replay_asset_manifest");
    let asset_manifest_sha256 = {
        let snapshot = backend
            .last_program_snapshot
            .as_ref()
            .ok_or_else(|| "AOT program snapshot missing during replay packaging".to_string())?;
        let result = (|| -> Result<Option<String>, String> {
            let resolved = crate::release_assets::resolve_snapshot_assets(project_dir, snapshot)?;
            if resolved.assets.is_empty() && resolved.dynamic_assets.is_empty() {
                return Ok(None);
            }
            prepare_asset_bundle(
                &resolved,
                &replay_asset_manifest_root,
                project_dir.join(".stasis_cache/assets"),
            )
            .map_err(|error| format!("failed to prepare desktop replay asset manifest: {error}"))?;
            let manifest_path = replay_asset_manifest_root.join(DEFAULT_ASSET_MANIFEST_PATH);
            let bytes = std::fs::read(&manifest_path).map_err(|error| {
                format!(
                    "failed to read desktop replay asset manifest {}: {error}",
                    manifest_path.display()
                )
            })?;
            Ok(Some(sha256_bytes(&bytes)))
        })();
        if replay_asset_manifest_root.exists() {
            std::fs::remove_dir_all(&replay_asset_manifest_root).map_err(|error| {
                format!(
                    "failed to clean desktop replay asset manifest {}: {error}",
                    replay_asset_manifest_root.display()
                )
            })?;
        }
        result?
    };
    let bindings_source = aot_root.join("published_aot_bindings.c");
    crate::mobile_aot_bindings::write_mobile_aot_bindings_source_with_profile_and_snapshot(
        &manifest_json,
        state_layout,
        project_dir,
        &bindings_source,
        &[],
        0,
        0,
        replay_state_snapshot.as_ref(),
    )?;
    std::fs::copy(
        aot_root.join("stasis_host_exports.h"),
        output_exe.with_extension("host_exports.h"),
    )
    .map_err(|error| format!("failed to publish host export header: {error}"))?;
    let replay_identity_header = aot_root.join("published_replay_identity.h");
    let replay_identity_source = aot_root.join("published_replay_identity.c");
    let portable_replay_compatibility = crate::packaged_replay_compatibility(
        backend
            .last_program_snapshot
            .as_ref()
            .ok_or_else(|| "AOT program snapshot missing during replay packaging".to_string())?,
        asset_manifest_sha256,
    )?;
    crate::mobile_aot_bindings::write_mobile_aot_replay_identity(
        &portable_replay_compatibility,
        &backend
            .last_program_snapshot
            .as_ref()
            .ok_or_else(|| "AOT program snapshot missing during replay packaging".to_string())?
            .replay_compatibility()
            .state_snapshot,
        replay_state_snapshot.is_some(),
        &replay_identity_header,
        &replay_identity_source,
    )?;
    let symbols_header = aot_root.join("published_aot_symbols.h");
    let replay_state_declarations = if replay_state_snapshot.is_some() {
        "int32_t stasis_replay_state_snapshot_size(void);\n\
         int32_t stasis_replay_state_snapshot_write(uint8_t *out, int32_t capacity);\n\
         int32_t stasis_replay_state_snapshot_restore(const uint8_t *input, int32_t bytes);\n"
    } else {
        ""
    };
    std::fs::write(
        &symbols_header,
        "#ifndef STASIS_PUBLISHED_AOT_SYMBOLS_H\n#define STASIS_PUBLISHED_AOT_SYMBOLS_H\n"
            .to_string()
            + "#include <stdint.h>\n"
            + "int32_t stasis_mobile_main_entry(void);\n"
            + "int32_t stasis_mobile_tick_entry(void);\n"
            + "int32_t stasis_mobile_render_entry(void);\n"
            + replay_state_declarations
            + "void stasis_aot_bind_runtime_globals(void);\n"
            + "#define STASIS_AOT_MAIN stasis_mobile_main_entry\n"
            + "#define STASIS_AOT_TICK stasis_mobile_tick_entry\n"
            + "#define STASIS_AOT_RENDER stasis_mobile_render_entry\n"
            + "#define STASIS_AOT_BIND_RUNTIME_GLOBALS stasis_aot_bind_runtime_globals\n"
            + "#endif\n",
    )
    .map_err(|error| format!("failed to write {}: {error}", symbols_header.display()))?;

    let mut object_list = String::from("set(STASIS_PUBLISHED_AOT_OBJECTS\n");
    for path in bundle.object_paths() {
        object_list.push_str(&format!("  \"{}\"\n", cmake_path(path)));
    }
    object_list.push_str(&format!("  \"{}\"\n", cmake_path(&replay_identity_source)));
    object_list.push_str(")\n");
    let object_list_path = aot_root.join("published_aot_objects.cmake");
    std::fs::write(&object_list_path, object_list)
        .map_err(|error| format!("failed to write {}: {error}", object_list_path.display()))?;

    let shell_template =
        std::fs::read_to_string(repo_root.join("mobile/shells/common/stasis_mobile_main.c"))
            .map_err(|error| format!("failed to read production host template: {error}"))?;
    let app_name = output_exe
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Stasis");
    let shell_source = shell_template
        .replace(
            "@STASIS_APP_NAME@",
            &crate::escape_mobile_c_string_literal(app_name),
        )
        .replace("@STASIS_ASSET_BASE@", ".");
    let shell_source_path = aot_root.join("stasis_desktop_main.c");
    std::fs::write(&shell_source_path, shell_source)
        .map_err(|error| format!("failed to write {}: {error}", shell_source_path.display()))?;

    let output_dir = output_exe.parent().unwrap_or_else(|| Path::new("."));
    let output_name = output_exe
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("invalid monolith output name {}", output_exe.display()))?;
    let build_dir = create_monolith_cmake_build_dir(&aot_root, output_exe)?;
    let configure_arguments = monolith_configure_arguments(
        &repo_root.join("runtime"),
        &build_dir.path,
        &aot_root,
        &shell_source_path,
        output_dir,
        output_name,
        desktop_network,
    );
    let configure = run_monolith_cmake(&configure_arguments)?;
    if !configure.status.success() {
        return Err(format!(
            "desktop monolith configure failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&configure.stdout),
            String::from_utf8_lossy(&configure.stderr)
        ));
    }
    let build = run_monolith_cmake(&[
        "--build".to_string(),
        cmake_path(&build_dir.path),
        "--config".to_string(),
        "Release".to_string(),
        "--target".to_string(),
        "stasis_monolith".to_string(),
    ])?;
    if !build.status.success() {
        return Err(format!(
            "desktop monolith build failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        ));
    }
    if !output_exe.is_file() {
        return Err(format!(
            "desktop monolith build did not produce {}",
            output_exe.display()
        ));
    }
    if cfg!(target_os = "macos") {
        let contents = output_exe
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| format!("invalid macOS monolith path {}", output_exe.display()))?;
        std::fs::create_dir_all(contents.join("Resources")).map_err(|error| {
            format!(
                "failed to create macOS app resources directory {}: {error}",
                contents.join("Resources").display()
            )
        })?;
        let executable_name = output_exe
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid macOS monolith name {}", output_exe.display()))?;
        write_macos_runner_info_plist(
            &contents.join("Info.plist"),
            executable_name,
            desktop_network.is_some_and(|network| network.mode == DesktopNetworkMode::Host),
        )?;
    }
    sign_output_artifact_if_configured(output_exe)?;
    if cfg!(target_os = "macos") {
        if let Some(app_bundle) = output_exe
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        {
            sign_output_artifact_if_configured(app_bundle)?;
        }
    }
    Ok(SelfHostedAotCliSummary {
        source_file_count: bundle.object_paths().count() + 1,
        linked_image_path: output_exe.to_path_buf(),
        entry_symbol: "main".to_string(),
        ir_bundle_path: PathBuf::new(),
        object_bundle_path: bundle.manifest_path.clone(),
        object_file_names: bundle
            .object_paths()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect(),
        program_snapshot: None,
    })
}

fn packaged_launch_sidecar_path(output_exe: &Path) -> Result<PathBuf, String> {
    let file_name = output_exe
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("invalid output file name {}", output_exe.display()))?;
    Ok(output_exe.with_file_name(format!("{file_name}.launch")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackagedRunnerLayout {
    executable: PathBuf,
    app_bundle: Option<PathBuf>,
    info_plist: Option<PathBuf>,
}

fn packaged_runner_layout(
    requested_output: &Path,
    macos_bundle: bool,
) -> Result<PackagedRunnerLayout, String> {
    if !macos_bundle {
        return Ok(PackagedRunnerLayout {
            executable: requested_output.to_path_buf(),
            app_bundle: None,
            info_plist: None,
        });
    }
    let requested_name = requested_output
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("invalid output file name {}", requested_output.display()))?;
    let (bundle, executable_name) = if requested_output.extension().is_some_and(|ext| ext == "app")
    {
        let executable_name = requested_output
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("invalid app bundle name {}", requested_output.display()))?;
        (requested_output.to_path_buf(), executable_name.to_string())
    } else {
        (
            requested_output.with_file_name(format!("{requested_name}.app")),
            requested_name.to_string(),
        )
    };
    let contents = bundle.join("Contents");
    Ok(PackagedRunnerLayout {
        executable: contents.join("MacOS").join(executable_name),
        info_plist: Some(contents.join("Info.plist")),
        app_bundle: Some(bundle),
    })
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_macos_runner_info_plist(
    path: &Path,
    executable_name: &str,
    local_network_host: bool,
) -> Result<(), String> {
    let mut bundle_component = String::new();
    for ch in executable_name.chars() {
        if ch.is_ascii_alphanumeric() {
            bundle_component.push(ch.to_ascii_lowercase());
        } else if !bundle_component.ends_with('-') {
            bundle_component.push('-');
        }
    }
    let bundle_component = bundle_component.trim_matches('-');
    let bundle_component = if bundle_component.is_empty() {
        "game"
    } else {
        bundle_component
    };
    let executable_name = xml_text(executable_name);
    let local_network_usage = if local_network_host {
        "    <key>NSLocalNetworkUsageDescription</key>\n    <string>Host browser guests on your local network.</string>\n"
    } else {
        ""
    };
    let contents = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>{executable_name}</string>
    <key>CFBundleIdentifier</key>
    <string>org.stasislang.game.{bundle_component}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{executable_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>NSHighResolutionCapable</key>
    <true/>
{local_network_usage}
</dict>
</plist>
"#
    );
    std::fs::write(path, contents).map_err(|error| {
        format!(
            "failed to write macOS app plist {}: {error}",
            path.display()
        )
    })
}

fn package_engine_bundle_release(
    backend: &mut IncrementalCompilerBackend,
    bundle: &AotEngineBundle,
    output_exe: &Path,
    project_dir: &Path,
    entry_file_override: Option<&Path>,
    desktop_network: Option<&DesktopNetworkLink>,
) -> Result<SelfHostedAotCliSummary, String> {
    let manifest = backend.read_engine_bundle_manifest(&bundle.manifest_path)?;
    let entry_symbol = resolve_engine_bundle_symbol(&manifest, "main")?;
    let tick_symbol = manifest
        .functions
        .iter()
        .find(|row| is_zero_argument_manifest_function(row, "tick"))
        .map(|row| row.symbol.clone());
    let render_row = manifest
        .functions
        .iter()
        .find(|row| is_zero_argument_manifest_function(row, "render"));
    let render_alias = render_row
        .map(|render| packaged_render_alias(&manifest, render))
        .transpose()?;

    let runner_layout = packaged_runner_layout(output_exe, cfg!(target_os = "macos"))?;
    let packaged_output_exe = &runner_layout.executable;
    let output_root = packaged_output_exe
        .parent()
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(output_root).map_err(|error| {
        format!(
            "failed to create AOT output directory {}: {error}",
            output_root.display()
        )
    })?;

    let monolithic_desktop = matches!(
        backend.aot_compile_config.target,
        stasis_jit::AotTarget::Native
    ) && (cfg!(windows) || desktop_network.is_some());
    let support_root = if monolithic_desktop {
        output_exe.parent().unwrap_or_else(|| Path::new("."))
    } else {
        output_root
    };
    let entry_file = resolve_self_host_aot_entry_file(project_dir, entry_file_override)?;
    let support = stage_entry_support_files(project_dir, entry_file.as_deref(), support_root)?;
    let state_layout = backend
        .last_program_snapshot
        .as_ref()
        .map(ProgramSnapshot::state_layout)
        .ok_or_else(|| "AOT program snapshot missing during packaging".to_string())?;
    let runtime_fields = merge_runtime_fields(state_layout, &support.runtime_fields)?;
    let replay_state_snapshot_supported = backend
        .last_program_snapshot
        .as_ref()
        .map(ProgramSnapshot::replay_compatibility)
        .map(|compatibility| replay_snapshot_bridge_can_emit(&compatibility.state_snapshot))
        .unwrap_or(false);
    if monolithic_desktop {
        return package_engine_bundle_monolithic_desktop(
            backend,
            bundle,
            packaged_output_exe,
            project_dir,
            desktop_network,
        );
    }
    let host_manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&bundle.manifest_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let host_exports = stasis_compiler::host_exports::HostExports::from_manifest(&host_manifest)?;
    std::fs::write(
        packaged_output_exe.with_extension("host_exports.h"),
        host_exports.header()?,
    )
    .map_err(|error| error.to_string())?;
    let mut function_aliases = vec![PackagedFunctionAlias {
        alias: "main",
        target_symbol: entry_symbol.clone(),
        returns_i32: true,
    }];
    if let Some(symbol) = tick_symbol.as_ref() {
        function_aliases.push(PackagedFunctionAlias {
            alias: "tick",
            target_symbol: symbol.clone(),
            returns_i32: true,
        });
    }

    let mut export_symbols: BTreeSet<String> = BTreeSet::new();
    export_symbols.extend(
        host_exports
            .functions
            .iter()
            .map(|record| record.symbol.clone()),
    );
    export_symbols.insert(entry_symbol.clone());
    export_symbols.insert("main".to_string());
    export_symbols.insert("stasis_aot_bind_runtime_globals".to_string());
    if replay_state_snapshot_supported {
        export_symbols.insert("stasis_replay_state_snapshot_size".to_string());
        export_symbols.insert("stasis_replay_state_snapshot_write".to_string());
        export_symbols.insert("stasis_replay_state_snapshot_restore".to_string());
    }
    if let Some(symbol) = tick_symbol.as_ref() {
        export_symbols.insert(symbol.clone());
        export_symbols.insert("tick".to_string());
    }
    if let Some(render) = render_row {
        export_symbols.insert(render.symbol.clone());
        export_symbols.insert("render".to_string());
    }
    for symbol in [
        "host_i32",
        "host_f32",
        "gfx_cmd_i32",
        "gfx_cmd_f32",
        "gfx_cmd_u8",
        "host_req_seq",
        "host_req_flags",
        "host_req_window_w_px",
        "host_req_window_h_px",
    ] {
        export_symbols.insert(format!("{symbol},DATA"));
    }
    for field in &runtime_fields {
        export_symbols.insert(format!("{},DATA", field.name));
    }

    let mut link_config = backend.aot_link_config.clone();
    link_config.target = backend.aot_compile_config.target.clone();
    let mut dynload_link_library = None;
    if should_link_stasis_dynload(&link_config.target) {
        let dynload_lib = ensure_stasis_dynload_link_library()?;
        if !link_config
            .runtime_lib_paths
            .iter()
            .any(|path| path == &dynload_lib)
        {
            link_config.runtime_lib_paths.push(dynload_lib.clone());
        }
        dynload_link_library = Some(dynload_lib);
    }
    if cfg!(windows) {
        if let Some(wrapper) = ensure_rust_lld_link_wrapper(&backend.aot_artifact_root) {
            link_config.linker_path = Some(wrapper);
        }
    }

    let linked_library_path =
        packaged_output_exe.with_extension(packaged_runtime_library_extension());
    let function_symbols: Vec<String> = manifest
        .functions
        .iter()
        .map(|row| row.symbol.clone())
        .collect();
    let string_literals = manifest.string_literals.clone().unwrap_or_default();
    let bridge_object = emit_engine_bundle_runtime_bridge_object(
        backend,
        &runtime_fields,
        &function_symbols,
        &function_aliases,
        render_alias.as_ref(),
        &string_literals,
        Some(&host_exports),
    )?;
    let mut object_paths: Vec<PathBuf> = bundle.object_paths().cloned().collect();
    object_paths.push(bridge_object);
    let export_symbols: Vec<String> = export_symbols.into_iter().collect();
    let initial_link = link_objects_to_dynamic_library(
        &object_paths,
        &linked_library_path,
        &export_symbols,
        &link_config,
    );
    if let Err(initial_error) = initial_link {
        if cfg!(windows) {
            if let Some(link_exe) = resolve_msvc_link_exe() {
                let mut fallback_config = link_config.clone();
                fallback_config.linker_path = Some(link_exe);
                link_objects_to_dynamic_library(
                    &object_paths,
                    &linked_library_path,
                    &export_symbols,
                    &fallback_config,
                )
                .map_err(|fallback_error| {
                    format!(
                        "dynamic library link failed with configured linker and MSVC fallback\nconfigured_link_error:\n{initial_error}\nmsvc_link_error:\n{fallback_error}"
                    )
                })?;
            } else {
                return Err(initial_error);
            }
        } else {
            return Err(initial_error);
        }
    }
    if let Some(link_library) = dynload_link_library.as_deref() {
        stage_stasis_dynload_runtime(link_library, &linked_library_path)?;
    }

    let (runner_src, graphics_src) = ensure_runtime_release_artifacts()?;
    eprintln!(
        "Stasis release runtime artifacts: runner={} graphics={}",
        runner_src.display(),
        graphics_src.display()
    );
    copy_file_creating_parent(&runner_src, packaged_output_exe)?;
    let graphics_dst = output_root.join(
        graphics_src
            .file_name()
            .ok_or_else(|| format!("invalid graphics runtime path {}", graphics_src.display()))?,
    );
    copy_file_creating_parent(&graphics_src, &graphics_dst)?;

    let launch_path = packaged_launch_sidecar_path(packaged_output_exe)?;
    let linked_library_name = linked_library_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            format!(
                "invalid linked library path {}",
                linked_library_path.display()
            )
        })?;
    let mut launch_lines = vec![
        format!("dll={linked_library_name}"),
        "entry=main".to_string(),
        "fps=60".to_string(),
    ];
    if tick_symbol.is_some() {
        launch_lines.push("tick=tick".to_string());
    }
    if render_row.is_some() {
        launch_lines.push("render=render".to_string());
        launch_lines.push(format!(
            "render_construction_lifecycle_version={}",
            manifest.render_construction_lifecycle_version
        ));
    }
    if let (Some(data_json), Some(data_meta)) = (
        support.data_bind_json_rel.as_ref(),
        support.data_bind_meta_rel.as_ref(),
    ) {
        launch_lines.push(format!("data_bind_json={data_json}"));
        launch_lines.push(format!("data_bind_meta={data_meta}"));
    }
    std::fs::write(&launch_path, launch_lines.join("\n")).map_err(|error| {
        format!(
            "failed to write launch manifest {}: {error}",
            launch_path.display()
        )
    })?;

    let import_lib_path = linked_library_path.with_extension("lib");
    if import_lib_path.exists() {
        std::fs::remove_file(&import_lib_path).map_err(|error| {
            format!(
                "failed to remove import library {}: {error}",
                import_lib_path.display()
            )
        })?;
    }
    let export_map_path = linked_library_path.with_extension("exp");
    if export_map_path.exists() {
        std::fs::remove_file(&export_map_path).map_err(|error| {
            format!(
                "failed to remove export map {}: {error}",
                export_map_path.display()
            )
        })?;
    }
    if let Some(info_plist) = runner_layout.info_plist.as_deref() {
        let executable_name = packaged_output_exe
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                format!(
                    "invalid packaged executable name {}",
                    packaged_output_exe.display()
                )
            })?;
        write_macos_runner_info_plist(info_plist, executable_name, false)?;
    }
    sign_output_artifact_if_configured(packaged_output_exe)?;
    sign_output_artifact_if_configured(&linked_library_path)?;
    sign_output_artifact_if_configured(&graphics_dst)?;
    if let Some(app_bundle) = runner_layout.app_bundle.as_deref() {
        sign_output_artifact_if_configured(app_bundle)?;
    }

    let object_file_names = object_paths
        .iter()
        .map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .collect();
    Ok(SelfHostedAotCliSummary {
        source_file_count: object_paths.len(),
        linked_image_path: packaged_output_exe.to_path_buf(),
        entry_symbol: "main".to_string(),
        ir_bundle_path: PathBuf::new(),
        object_bundle_path: bundle.manifest_path.clone(),
        object_file_names,
        program_snapshot: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_host_aot_artifact_root_override_is_package_scoped() {
        let project_dir = Path::new(r"C:\captured\.source-snapshot");
        let output_exe = project_dir.join(".package-output/game.exe");
        let normal = resolve_self_host_aot_artifact_root(project_dir, &output_exe, None);
        assert_eq!(
            normal,
            project_dir.join(".stasis_cache/aot_cli/game"),
            "ordinary AOT builds retain their project cache location"
        );

        let temporary = PathBuf::from(r"C:\Temp\stasis-pkg-aot-123-456");
        let overridden =
            resolve_self_host_aot_artifact_root(project_dir, &output_exe, Some(&temporary));
        assert_eq!(
            overridden, temporary,
            "package AOT builds must use the explicit short-lived artifact root"
        );
        assert!(
            !overridden.starts_with(project_dir),
            "the package override must not be nested below the captured source snapshot"
        );
    }

    #[test]
    fn desktop_monolith_network_configuration_is_explicit_and_optional() {
        let base = monolith_configure_arguments(
            Path::new("runtime"),
            Path::new("build"),
            Path::new("aot"),
            Path::new("main.c"),
            Path::new("dist"),
            "game",
            None,
        );
        assert!(!base.iter().any(|arg| arg.contains("NETWORK")));
        assert!(base.iter().any(|arg| arg == "-DCMAKE_BUILD_TYPE=Release"));

        let network = DesktopNetworkLink {
            library: PathBuf::from("network/stasis_network.lib"),
            include_dir: PathBuf::from("network/include"),
            mode: DesktopNetworkMode::Host,
        };
        let configured = monolith_configure_arguments(
            Path::new("runtime"),
            Path::new("build"),
            Path::new("aot"),
            Path::new("main.c"),
            Path::new("dist"),
            "game",
            Some(&network),
        );
        assert!(configured
            .iter()
            .any(|arg| arg == "-DSTASIS_MONOLITH_NETWORK_LIBRARY=network/stasis_network.lib"));
        assert!(configured
            .iter()
            .any(|arg| arg == "-DSTASIS_MONOLITH_NETWORK_INCLUDE_DIR=network/include"));
        assert!(configured
            .iter()
            .any(|arg| arg == "-DSTASIS_MONOLITH_NETWORK_MODE=host"));

        let client = DesktopNetworkLink {
            library: PathBuf::from("network/stasis_network.lib"),
            include_dir: PathBuf::from("network/include"),
            mode: DesktopNetworkMode::Client,
        };
        let configured = monolith_configure_arguments(
            Path::new("runtime"),
            Path::new("build"),
            Path::new("aot"),
            Path::new("main.c"),
            Path::new("dist"),
            "game",
            Some(&client),
        );
        assert!(configured
            .iter()
            .any(|arg| arg == "-DSTASIS_MONOLITH_NETWORK_MODE=client"));
    }

    #[test]
    fn desktop_network_shell_keeps_private_join_links_behind_explicit_copy() {
        let source = include_str!("../../../mobile/shells/common/stasis_mobile_main.c")
            .replace("\r\n", "\n");
        assert!(
            source.contains("defined(STASIS_DESKTOP_MONOLITH) && defined(STASIS_NETWORK_ENABLED)")
        );
        let explicit_copy = source
            .find("if (SDL_ShowMessageBox(&card, &button) && button == 1)")
            .expect("private URL copy must require the native copy button");
        let private_url = source[explicit_copy..]
            .find("stasis_network_copy_private_join_url(")
            .expect("explicit action must copy through the wiping helper");
        assert!(private_url > 0);
        assert!(source.contains("#if defined(STASIS_DESKTOP_MONOLITH)\nint main("));
        assert!(source.contains("SDL_SetMainReady();"));
        assert!(source.contains("network_join_shortcut_down"));
        assert!(source.contains("snprintf(path, sizeof(path), \"%s../../../\", base)"));
    }

    #[test]
    fn engine_manifest_accepts_versioned_hot_render_metadata() {
        assert_eq!(
            stasis_compiler::backend::hot_render::HOT_RENDER_METADATA_VERSION,
            stasis_dynload::HOT_RENDER_METADATA_VERSION
        );
        let manifest: EngineBundleManifest = serde_json::from_str(
            r#"{"functions":[],"hot_render_metadata_version":4,"hot_render_images":[{"logical_path":"assets/hero.png","logical_width":32,"logical_height":24,"max_renders_per_render":3,"atlas_eligible":true,"grouping_key":"batch-v3:test","estimated_distinct_transitions":4,"group_member_count":2,"group_logical_pixel_area":1536,"group_max_logical_width":32,"group_max_logical_height":24,"backend_constraints":"desktop-gl"}]}"#,
        )
        .expect("parse hot-render manifest");
        assert_eq!(manifest.hot_render_metadata_version, Some(4));
        let image = &manifest.hot_render_images.expect("image rows")[0];
        assert_eq!(image.max_renders_per_render, Some(3));
        assert!(image.atlas_eligible);
        assert_eq!(image.backend_constraints.as_deref(), Some("desktop-gl"));
    }

    #[test]
    fn engine_manifest_v4_pair_evidence_reaches_host_policy() {
        let manifest: EngineBundleManifest = serde_json::from_str(
            r#"{"functions":[],"hot_render_metadata_version":4,"hot_render_images":[{"identity":"hero","logical_path":"assets/hero.png","logical_width":32,"logical_height":24,"max_renders_per_render":3,"atlas_eligible":true,"grouping_key":"batch-v3:test","backend_constraints":"desktop-gl"},{"identity":"enemy","logical_path":"assets/enemy.png","logical_width":32,"logical_height":24,"max_renders_per_render":3,"atlas_eligible":true,"grouping_key":"batch-v3:test","backend_constraints":"desktop-gl"}],"hot_render_transitions":[{"from_identity":"hero","to_identity":"enemy","max_transitions_per_render":4,"validity":"finite","provenance":["sequence_join"]}],"hot_render_transition_analysis":{"validity":"complete","unknown_causes":[],"pair_count":1,"published_pair_count":1,"omitted_pair_count":0,"pair_limit":4096,"max_pair_weight":1000000000}}"#,
        )
        .expect("parse v4 manifest");
        let policy = manifest_hot_render_policy(&manifest);
        assert_eq!(policy.version, stasis_dynload::HOT_RENDER_METADATA_VERSION);
        assert_eq!(policy.images.len(), 2);
        assert_eq!(policy.transitions.len(), 1);
        assert_eq!(policy.transitions[0].max_transitions_per_render, Some(4));
        assert_eq!(policy.transitions[0].provenance, ["sequence_join"]);
        assert_eq!(policy.analysis.validity, "complete");
        assert_eq!(policy.analysis.omitted_pair_count, Some(0));
    }
    #[test]
    fn packaged_frame_callbacks_require_zero_arguments() {
        let manifest: EngineBundleManifest = serde_json::from_str(
            r#"{"functions":[{"function_id":1,"symbol_id":"render-indexed","name":"render","symbol":"render_indexed","return_type":1,"parameter_count":1},{"function_id":2,"symbol_id":"render-frame","name":"render","symbol":"render_frame","return_type":1,"parameter_count":0}]}"#,
        )
        .expect("parse callback manifest");
        let render = manifest
            .functions
            .iter()
            .find(|row| is_zero_argument_manifest_function(row, "render"))
            .expect("zero-argument render callback");
        assert_eq!(render.symbol, "render_frame");
        let alias = packaged_render_alias(&manifest, render).expect("legacy render alias");
        assert!(alias.reset_symbol.is_none());
        assert!(alias.finish_symbol.is_none());
        assert!(alias.returns_i32);
        assert!(!is_zero_argument_manifest_function(
            &manifest.functions[0],
            "render"
        ));
    }

    #[test]
    fn packaged_render_alias_validates_manifest_return_and_lifecycle_contracts() {
        let manifest: EngineBundleManifest = serde_json::from_str(
            r#"{"render_construction_lifecycle_version":1,"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":0,"parameter_count":0},{"function_id":2,"symbol_id":"reset","name":"gfx_cmd_construction_reset","symbol":"aot_reset","return_type":0,"parameter_count":0},{"function_id":3,"symbol_id":"finish","name":"gfx_cmd_construction_finish","symbol":"aot_finish","return_type":1,"parameter_count":1}]}"#,
        )
        .expect("parse lifecycle manifest");
        let render = &manifest.functions[0];
        let alias = packaged_render_alias(&manifest, render).expect("valid lifecycle alias");
        assert!(!alias.returns_i32);
        assert_eq!(alias.reset_symbol.as_deref(), Some("aot_reset"));
        assert_eq!(alias.finish_symbol.as_deref(), Some("aot_finish"));

        let malformed: EngineBundleManifest = serde_json::from_str(
            r#"{"render_construction_lifecycle_version":1,"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":1,"parameter_count":0},{"function_id":2,"symbol_id":"reset","name":"gfx_cmd_construction_reset","symbol":"aot_reset","return_type":1,"parameter_count":0},{"function_id":3,"symbol_id":"finish","name":"gfx_cmd_construction_finish","symbol":"aot_finish","return_type":1,"parameter_count":1}]}"#,
        )
        .expect("parse malformed lifecycle manifest");
        assert!(packaged_render_alias(&malformed, &malformed.functions[0])
            .expect_err("wrong reset signature must fail")
            .contains("must have signature void()"));

        let unsupported: EngineBundleManifest = serde_json::from_str(
            r#"{"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":2,"parameter_count":0}]}"#,
        )
        .expect("parse unsupported render manifest");
        assert!(
            packaged_render_alias(&unsupported, &unsupported.functions[0])
                .expect_err("unsupported render result must fail")
                .contains("must return void or i32")
        );

        let partial: EngineBundleManifest = serde_json::from_str(
            r#"{"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":1,"parameter_count":0},{"function_id":2,"symbol_id":"reset","name":"gfx_cmd_construction_reset","symbol":"aot_reset","return_type":0,"parameter_count":0}]}"#,
        )
        .expect("parse version-zero partial lifecycle manifest");
        assert!(packaged_render_alias(&partial, &partial.functions[0])
            .expect_err("version-zero partial lifecycle must fail")
            .contains("requires both reset and finish helpers"));

        let duplicate: EngineBundleManifest = serde_json::from_str(
            r#"{"render_construction_lifecycle_version":1,"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":1,"parameter_count":0},{"function_id":2,"symbol_id":"reset-1","name":"gfx_cmd_construction_reset","symbol":"aot_reset_1","return_type":0,"parameter_count":0},{"function_id":3,"symbol_id":"reset-2","name":"gfx_cmd_construction_reset","symbol":"aot_reset_2","return_type":0,"parameter_count":0},{"function_id":4,"symbol_id":"finish-1","name":"gfx_cmd_construction_finish","symbol":"aot_finish_1","return_type":1,"parameter_count":1},{"function_id":5,"symbol_id":"finish-2","name":"gfx_cmd_construction_finish","symbol":"aot_finish_2","return_type":1,"parameter_count":1}]}"#,
        )
        .expect("parse duplicate lifecycle manifest");
        assert!(packaged_render_alias(&duplicate, &duplicate.functions[0])
            .expect_err("duplicate lifecycle helpers must fail")
            .contains("requires exactly one reset and finish helper"));

        let legacy_with_helpers: EngineBundleManifest = serde_json::from_str(
            r#"{"render_construction_lifecycle_version":0,"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":1,"parameter_count":0},{"function_id":2,"symbol_id":"reset","name":"gfx_cmd_construction_reset","symbol":"aot_reset","return_type":0,"parameter_count":0},{"function_id":3,"symbol_id":"finish","name":"gfx_cmd_construction_finish","symbol":"aot_finish","return_type":1,"parameter_count":1}]}"#,
        )
        .expect("parse legacy lifecycle manifest");
        let legacy_alias =
            packaged_render_alias(&legacy_with_helpers, &legacy_with_helpers.functions[0])
                .expect_err("version-zero lifecycle must reject construction helpers");
        assert!(legacy_alias.contains("version 0 must omit construction helpers"));

        let legacy_without_helpers: EngineBundleManifest = serde_json::from_str(
            r#"{"render_construction_lifecycle_version":0,"functions":[{"function_id":1,"symbol_id":"render","name":"render","symbol":"aot_render","return_type":1,"parameter_count":0}]}"#,
        )
        .expect("parse legacy direct manifest");
        let legacy_alias = packaged_render_alias(
            &legacy_without_helpers,
            &legacy_without_helpers.functions[0],
        )
        .expect("version-zero lifecycle without helpers remains direct");
        assert!(legacy_alias.reset_symbol.is_none());
        assert!(legacy_alias.finish_symbol.is_none());
    }

    #[test]
    fn packaged_render_alias_wraps_frame_construction() {
        let render = PackagedRenderAlias {
            target_symbol: "aot_render".to_string(),
            returns_i32: true,
            reset_symbol: Some("aot_reset".to_string()),
            finish_symbol: Some("aot_finish".to_string()),
        };
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &[],
            &[
                "aot_render".to_string(),
                "aot_reset".to_string(),
                "aot_finish".to_string(),
            ],
            &[],
            Some(&render),
            &[],
        )
        .expect("build render bridge source");
        let wrapper = source
            .lines()
            .find(|line| line.starts_with("STASIS_EXPORT int32_t render(void)"))
            .expect("render wrapper");
        let reset = wrapper.find("aot_reset").expect("reset call");
        let render = wrapper.find("aot_render").expect("render call");
        let finish = wrapper.find("aot_finish").expect("finish call");
        assert!(reset < render && render < finish);
        assert!(wrapper.contains("int32_t result"));
    }

    #[test]
    fn packaged_render_alias_preserves_void_return_contract() {
        let render = PackagedRenderAlias {
            target_symbol: "aot_render".to_string(),
            returns_i32: false,
            reset_symbol: Some("aot_reset".to_string()),
            finish_symbol: Some("aot_finish".to_string()),
        };
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &[],
            &[
                "aot_render".to_string(),
                "aot_reset".to_string(),
                "aot_finish".to_string(),
            ],
            &[],
            Some(&render),
            &[],
        )
        .expect("build void render bridge source");
        let wrapper = source
            .lines()
            .find(|line| line.starts_with("STASIS_EXPORT int32_t render(void)"))
            .expect("render wrapper");
        assert!(wrapper.contains("((void (*)(void))aot_render)();"));
        assert!(wrapper.contains("aot_finish)(0)"));
        assert!(!wrapper.contains("int32_t result"));

        let legacy = PackagedRenderAlias {
            target_symbol: "aot_render".to_string(),
            returns_i32: false,
            reset_symbol: None,
            finish_symbol: None,
        };
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &[],
            &["aot_render".to_string()],
            &[],
            Some(&legacy),
            &[],
        )
        .expect("build legacy void render bridge source");
        let wrapper = source
            .lines()
            .find(|line| line.starts_with("STASIS_EXPORT int32_t render(void)"))
            .expect("legacy render wrapper");
        assert!(wrapper.contains("((void (*)(void))aot_render)(); return 0;"));
        assert!(!wrapper.contains("aot_finish"));
    }

    #[test]
    fn macos_packaged_runner_keeps_executable_and_retina_plist_in_app_bundle() {
        let requested = Path::new("dist").join("Chess TD");
        let layout = packaged_runner_layout(&requested, true).expect("macOS runner layout");
        assert_eq!(layout.app_bundle, Some(PathBuf::from("dist/Chess TD.app")));
        assert_eq!(
            layout.executable,
            PathBuf::from("dist/Chess TD.app/Contents/MacOS/Chess TD")
        );
        assert_eq!(
            layout.info_plist,
            Some(PathBuf::from("dist/Chess TD.app/Contents/Info.plist"))
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_macos_runner_plist_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create plist test directory");
        let plist = temp_root.join("Info.plist");
        write_macos_runner_info_plist(&plist, "Chess & TD", true).expect("write macOS app plist");
        let contents = fs::read_to_string(&plist).expect("read macOS app plist");
        assert!(contents.contains("<key>NSHighResolutionCapable</key>\n    <true/>"));
        assert!(contents.contains("<string>Chess &amp; TD</string>"));
        assert!(contents.contains("<string>org.stasislang.game.chess-td</string>"));
        assert!(contents.contains("<key>NSLocalNetworkUsageDescription</key>"));
        let client_plist = temp_root.join("ClientInfo.plist");
        write_macos_runner_info_plist(&client_plist, "Chess & TD", false)
            .expect("write macOS client app plist");
        let client_contents = fs::read_to_string(&client_plist).expect("read client app plist");
        assert!(!client_contents.contains("NSLocalNetworkUsageDescription"));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn macos_packaged_runner_accepts_an_explicit_app_output() {
        let requested = Path::new("dist").join("ChessTD.app");
        let layout = packaged_runner_layout(&requested, true).expect("macOS runner layout");
        assert_eq!(layout.app_bundle, Some(requested));
        assert_eq!(
            layout.executable,
            PathBuf::from("dist/ChessTD.app/Contents/MacOS/ChessTD")
        );
    }

    fn snapshot_semantic_fingerprint(snapshot: &ProgramSnapshot) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
            snapshot.source_revision(),
            snapshot.functions(),
            snapshot.state_layout(),
            snapshot.literal_table(),
            snapshot.types(),
            snapshot.data_flow_summaries(),
            snapshot.artifact_mappings(),
        )
    }

    #[test]
    fn runner_diagnostic_uses_second_file_source_span() {
        let _global_guard = crate::jit_test_support::lock();
        let mut backend = IncrementalCompilerBackend::new();
        backend.source_by_path.insert(
            "main.stasis".to_string(),
            "function main(): i32 { return helper(); }".to_string(),
        );
        backend.source_by_path.insert(
            "dep.stasis".to_string(),
            "\nfunction helper(): i32 { return missing(); }".to_string(),
        );
        let diagnostic = backend.runner_diagnostic_from_source(
            Some(&stasis_compiler::SourceDiagnostic::new(
                "dep.stasis",
                1,
                9,
                "helper",
                "unknown call target",
            )),
            "fallback".to_string(),
            Some(PathBuf::from("main.stasis")),
        );
        assert_eq!(diagnostic.path, Some(PathBuf::from("dep.stasis")));
        assert_eq!(diagnostic.line, Some(2));
        assert_eq!(diagnostic.column, Some(1));
        assert_eq!(diagnostic.code.as_deref(), Some("stasis.generic"));
    }

    fn assert_second_file_diagnostic(target_mode: TargetMode) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_snapshot_diagnostic_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let main = temp_root.join("main.stasis");
        let dependency = temp_root.join("dependency.stasis");
        fs::write(
            &main,
            "import \"dependency.stasis\"; function main(): i32 { return helper(); }\n",
        )
        .expect("write main");
        fs::write(
            &dependency,
            "\nfunction helper(): i32 { return missing(); }\n",
        )
        .expect("write dependency");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(98_001),
            vec![main, dependency.clone()],
            target_mode,
        ));
        assert_eq!(result.status, CompileStatus::Failed, "{result:?}");
        let diagnostic = result.diagnostics.first().expect("source diagnostic");
        assert_eq!(diagnostic.path, Some(dependency));
        assert_eq!(diagnostic.line, Some(2));
        assert_eq!(diagnostic.column, Some(24));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_rejection_reports_imported_file_source_span() {
        let _global_guard = crate::jit_test_support::lock();
        assert_second_file_diagnostic(TargetMode::JitDev);
    }

    #[test]
    fn explicit_project_root_keeps_identity_stable_when_new_directory_is_added() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_fixed_project_root_{stamp}"));
        let src = temp_root.join("src");
        let tests = temp_root.join("tests");
        fs::create_dir_all(&src).expect("create src");
        fs::create_dir_all(&tests).expect("create tests");
        let main = src.join("main.stasis");
        let added_test = tests.join("identity.test.stasis");
        fs::write(&main, "function main(): i32 { return 7; }\n").expect("write main");

        let mut jit = IncrementalCompilerBackend::new_for_project(&temp_root)
            .expect("create rooted JIT backend");
        assert_eq!(
            jit.compile(CompileRequest::new(
                RequestId(98_005),
                vec![main.clone()],
                TargetMode::JitDev,
            ))
            .status,
            CompileStatus::Success
        );
        let initial = jit
            .last_program_snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .functions()
                    .iter()
                    .find(|function| function.name == "main")
            })
            .map(|function| (function.symbol_id.clone(), function.id))
            .expect("initial main identity");

        fs::write(
            &added_test,
            "test `identity remains stable`(): bool { return true; }\n",
        )
        .expect("write added test");
        assert_eq!(
            jit.compile(CompileRequest::new(
                RequestId(98_006),
                vec![added_test.clone()],
                TargetMode::JitDev,
            ))
            .status,
            CompileStatus::Success
        );
        let after_addition = jit
            .last_program_snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .functions()
                    .iter()
                    .find(|function| function.name == "main")
            })
            .map(|function| (function.symbol_id.clone(), function.id))
            .expect("updated main identity");
        assert_eq!(after_addition, initial);
        assert!(initial.0.canonical().contains("|src/main.stasis|main|"));

        let mut aot = IncrementalCompilerBackend::new_for_project(&temp_root)
            .expect("create rooted AOT backend");
        assert_eq!(
            aot.compile(CompileRequest::new(
                RequestId(98_007),
                vec![main, added_test],
                TargetMode::AotProd,
            ))
            .status,
            CompileStatus::Success
        );
        let aot_identity = aot
            .last_program_snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .functions()
                    .iter()
                    .find(|function| function.name == "main")
            })
            .map(|function| (function.symbol_id.clone(), function.id))
            .expect("AOT main identity");
        assert_eq!(aot_identity, initial);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_rejection_reports_imported_file_source_span() {
        let _global_guard = crate::jit_test_support::lock();
        assert_second_file_diagnostic(TargetMode::AotProd);
    }

    #[test]
    fn rejected_jit_parse_preserves_accepted_snapshot() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_snapshot_jit_rollback_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 1; }\n").expect("write valid source");
        let mut backend = IncrementalCompilerBackend::new();
        assert_eq!(
            backend
                .compile(CompileRequest::new(
                    RequestId(98_010),
                    vec![source.clone()],
                    TargetMode::JitDev
                ))
                .status,
            CompileStatus::Success
        );
        let accepted = snapshot_semantic_fingerprint(
            backend
                .last_program_snapshot
                .as_ref()
                .expect("accepted snapshot"),
        );
        fs::write(&source, "function main(: i32 { return 2; }\n").expect("write invalid source");
        assert_eq!(
            backend
                .compile(CompileRequest::new(
                    RequestId(98_011),
                    vec![source],
                    TargetMode::JitDev
                ))
                .status,
            CompileStatus::Failed
        );
        assert_eq!(
            snapshot_semantic_fingerprint(
                backend
                    .last_program_snapshot
                    .as_ref()
                    .expect("preserved snapshot"),
            ),
            accepted
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn rejected_generic_expansion_preserves_prepared_jit_snapshot_package_and_queue() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_generic_jit_rollback_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "struct Buffer<N: i32> { value: i32; }\n\
             global buffer: Buffer<4>;\n\
             function capacity(self: Buffer<N>): i32 { return N; }\n\
             function main(): i32 { return buffer.capacity(); }\n\
             function tick(): i32 { return buffer.capacity(); }\n\
             function render(): i32 { return 0; }\n\
             function on_code_swap(): void { return; }\n",
        )
        .expect("write valid generic engine source");

        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let mut backend = IncrementalCompilerBackend::new_with_prepared_jit_swaps(sender);
        let baseline = backend.compile(CompileRequest::new(
            RequestId(98_012),
            vec![source.clone()],
            TargetMode::JitDev,
        ));
        assert_eq!(baseline.status, CompileStatus::Success, "{baseline:?}");
        let prepared = receiver.recv().expect("prepared baseline candidate");
        assert_eq!(prepared.candidate.execute_i32_noarg_by_name("main"), Ok(4));
        let accepted_snapshot = snapshot_semantic_fingerprint(
            backend
                .last_program_snapshot
                .as_ref()
                .expect("accepted snapshot"),
        );
        let accepted_package = backend
            .last_jit_engine_package
            .as_ref()
            .expect("accepted engine package")
            .clone();
        assert!(backend.pending_jit_candidate.is_none());

        fs::write(
            &source,
            "struct Node<N: i32> { next: Node<N + 1>; }\n\
             global root: Node<0>;\n\
             function main(): i32 { return 0; }\n\
             function tick(): i32 { return 0; }\n\
             function render(): i32 { return 0; }\n\
             function on_code_swap(): void { return; }\n",
        )
        .expect("write expanding generic candidate");
        let rejected = backend.compile(CompileRequest::new(
            RequestId(98_013),
            vec![source.clone()],
            TargetMode::JitDev,
        ));
        assert_eq!(rejected.status, CompileStatus::Failed, "{rejected:?}");
        assert!(rejected.diagnostics.iter().any(|diagnostic| {
            diagnostic.code.as_deref() == Some("stasis.generic")
                && diagnostic
                    .message
                    .contains("generic instantiation depth exceeded")
        }));
        assert_eq!(
            snapshot_semantic_fingerprint(
                backend
                    .last_program_snapshot
                    .as_ref()
                    .expect("preserved snapshot"),
            ),
            accepted_snapshot
        );
        assert_eq!(
            backend
                .last_jit_engine_package
                .as_ref()
                .expect("preserved engine package"),
            &accepted_package
        );
        assert!(backend.pending_jit_candidate.is_none());
        assert!(matches!(
            receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn failed_prepared_jit_send_preserves_accepted_snapshot() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_snapshot_send_rollback_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 1; }\n").expect("write source");
        let (live_sender, live_receiver) = std::sync::mpsc::sync_channel(1);
        let mut backend = IncrementalCompilerBackend::new_with_prepared_jit_swaps(live_sender);
        assert_eq!(
            backend
                .compile(CompileRequest::new(
                    RequestId(98_020),
                    vec![source.clone()],
                    TargetMode::JitDev
                ))
                .status,
            CompileStatus::Success
        );
        let accepted_candidate = live_receiver.recv().expect("prepared accepted baseline");
        let accepted = snapshot_semantic_fingerprint(
            backend
                .last_program_snapshot
                .as_ref()
                .expect("accepted snapshot"),
        );
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);
        drop(receiver);
        backend.prepared_jit_swap_tx = Some(sender);
        fs::write(&source, "function main(): i32 { return 2; }\n").expect("update source");
        let result = backend.compile(CompileRequest::new(
            RequestId(98_021),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert_eq!(
            snapshot_semantic_fingerprint(
                backend
                    .last_program_snapshot
                    .as_ref()
                    .expect("preserved snapshot"),
            ),
            accepted
        );
        assert_eq!(
            accepted_candidate
                .candidate
                .execute_i32_noarg_by_name("main"),
            Ok(1)
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn prepared_jit_rejection_reports_second_file_candidate_diagnostic() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_prepared_diagnostic_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let main = temp_root.join("main.stasis");
        let dependency = temp_root.join("dependency.stasis");
        fs::write(
            &main,
            "import \"dependency.stasis\"; function main(): i32 { return helper(); }\n",
        )
        .expect("write main");
        fs::write(&dependency, "\nfunction helper(): i32 { return 1; }\n")
            .expect("write dependency");
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let mut backend = IncrementalCompilerBackend::new_with_prepared_jit_swaps(sender);
        assert_eq!(
            backend
                .compile(CompileRequest::new(
                    RequestId(98_025),
                    vec![main.clone(), dependency.clone()],
                    TargetMode::JitDev
                ))
                .status,
            CompileStatus::Success
        );
        receiver.recv().expect("prepared baseline");
        fs::write(
            &dependency,
            "\nfunction helper(): i32 { return missing(); }\n",
        )
        .expect("write invalid dependency");
        let result = backend.compile(CompileRequest::new(
            RequestId(98_026),
            vec![dependency.clone()],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        let diagnostic = result.diagnostics.first().expect("candidate diagnostic");
        assert_eq!(diagnostic.path, Some(dependency));
        assert_eq!(diagnostic.line, Some(2));
        assert_eq!(diagnostic.column, Some(24));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_write_fault_preserves_accepted_snapshot_and_bundle() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_snapshot_aot_rollback_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "function tick(): i32 { return 1; }\nfunction render(): i32 { return 2; }\n",
        )
        .expect("write source");
        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            temp_root.join("artifacts"),
        );
        assert_eq!(
            backend
                .compile(CompileRequest::new(
                    RequestId(98_030),
                    vec![source.clone()],
                    TargetMode::AotProd
                ))
                .status,
            CompileStatus::Success
        );
        let accepted = snapshot_semantic_fingerprint(
            backend
                .last_program_snapshot
                .as_ref()
                .expect("accepted snapshot"),
        );
        let accepted_bundle = backend
            .last_aot_engine_bundle
            .as_ref()
            .expect("accepted bundle")
            .clone();
        let accepted_manifest_text =
            fs::read_to_string(&accepted_bundle.manifest_path).expect("read accepted manifest");
        let blocked_root = temp_root.join("blocked-root");
        fs::write(&blocked_root, "not a directory").expect("write blocking file");
        backend.aot_artifact_root = blocked_root;
        let result = backend.compile(CompileRequest::new(
            RequestId(98_031),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert_eq!(
            snapshot_semantic_fingerprint(
                backend
                    .last_program_snapshot
                    .as_ref()
                    .expect("preserved snapshot"),
            ),
            accepted
        );
        let preserved_bundle = backend
            .last_aot_engine_bundle
            .as_ref()
            .expect("preserved bundle");
        assert_eq!(preserved_bundle, &accepted_bundle);
        assert_eq!(
            fs::read_to_string(&preserved_bundle.manifest_path).expect("read preserved manifest"),
            accepted_manifest_text
        );
        assert!(preserved_bundle
            .object_paths_by_function
            .values()
            .all(|path| path.exists()));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn successful_aot_snapshot_mappings_reference_existing_objects() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_snapshot_aot_mappings_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 1; }\n").expect("write source");
        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            temp_root.join("artifacts"),
        );
        assert_eq!(
            backend
                .compile(CompileRequest::new(
                    RequestId(98_040),
                    vec![source],
                    TargetMode::AotProd
                ))
                .status,
            CompileStatus::Success
        );
        let snapshot = backend
            .last_program_snapshot
            .as_ref()
            .expect("AOT snapshot");
        assert!(!snapshot.artifact_mappings().is_empty());
        for mapping in snapshot.artifact_mappings().values() {
            assert!(Path::new(
                mapping
                    .target_path
                    .as_deref()
                    .expect("materialized object path")
            )
            .exists());
        }
        fs::remove_dir_all(&temp_root).ok();
    }
    #[cfg(windows)]
    use object::{Object, ObjectSection};
    #[cfg(unix)]
    use stasis_dynload::{
        invoke_i32_i32_i32_to_i32, invoke_i32_to_i32, invoke_noarg_i32, Library as DynamicLibrary,
    };
    #[cfg(windows)]
    use stasis_dynload::{invoke_noarg_u64, Library as DynamicLibrary};
    use stasis_runner::swap::contracts::{CompileRequest, CompileStatus, RequestId, TargetMode};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static SIGN_ENV_LOCK: Mutex<()> = Mutex::new(());
    static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn stasis_process_env_lock() -> &'static Mutex<()> {
        &PROCESS_ENV_LOCK
    }

    struct RemovedEnvironmentVariable {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl RemovedEnvironmentVariable {
        fn new(name: &'static str) -> Self {
            let previous = std::env::var_os(name);
            std::env::remove_var(name);
            Self { name, previous }
        }
    }

    impl Drop for RemovedEnvironmentVariable {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                std::env::set_var(self.name, value);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn disable_ambient_signing() -> Vec<RemovedEnvironmentVariable> {
        // Fake-linker fixtures emit text, never executable images.
        [
            "STASIS_AOT_SIGN_TOOL",
            "STASIS_REQUIRE_SIGNED_EXECUTION",
            "STASIS_SIGNING_MODE",
            "STASIS_SIGNING_PROFILE",
            "STASIS_SIGNING_CERTIFICATE",
            "STASIS_SIGNING_CERT_THUMBPRINT",
            "STASIS_SIGNING_LOCAL_RECORD",
            "LOCALAPPDATA",
        ]
        .into_iter()
        .map(RemovedEnvironmentVariable::new)
        .collect()
    }

    #[test]
    fn aot_direct_storage_source_uses_enum_i32_lanes() {
        let layout = StateLayout {
            scalars: vec![stasis_compiler::backend::state_layout::StateScalarLayout {
                path: "game.phase".to_string(),
                type_name: "GamePhase".to_string(),
                storage_type_name: "i32".to_string(),
            }],
            collections: vec![
                stasis_compiler::backend::state_layout::StateCollectionLayout {
                    path: "game.samples".to_string(),
                    capacity: 2,
                    element_shape: "GamePhase".to_string(),
                    fully_migratable: false,
                    fields: vec![
                        stasis_compiler::backend::state_layout::StateCollectionFieldLayout {
                            field: String::new(),
                            type_name: "GamePhase".to_string(),
                            storage_type_name: "i32".to_string(),
                            element_count: None,
                        },
                    ],
                },
            ],
            structs: Vec::new(),
            opaque: Vec::new(),
        };

        let (source, register_lines) =
            build_aot_direct_storage_source(&layout).expect("build enum storage source");

        assert!(source.contains("STASIS_EXPORT int32_t stasis_state_scalar__game__phase = 0;"));
        assert!(
            source.contains("STASIS_EXPORT int32_t stasis_state_array__game__samples[2] = {0};")
        );
        assert!(register_lines
            .iter()
            .any(|line| line.contains("stasis_jit_register_global_i32_ptr")));
        assert!(register_lines
            .iter()
            .any(|line| line.contains("stasis_jit_register_global_i32_array")));
    }

    #[test]
    fn aot_direct_storage_source_separates_collection_metadata_and_field_symbols() {
        let layout = StateLayout {
            scalars: vec![
                stasis_compiler::backend::state_layout::StateScalarLayout {
                    path: "game.walls.length".to_string(),
                    type_name: "i32".to_string(),
                    storage_type_name: "i32".to_string(),
                },
                stasis_compiler::backend::state_layout::StateScalarLayout {
                    path: "game.walls.max_length".to_string(),
                    type_name: "i32".to_string(),
                    storage_type_name: "i32".to_string(),
                },
            ],
            collections: vec![
                stasis_compiler::backend::state_layout::StateCollectionLayout {
                    path: "game.walls".to_string(),
                    capacity: 2,
                    element_shape: "WallRun".to_string(),
                    fully_migratable: false,
                    fields: vec![
                        stasis_compiler::backend::state_layout::StateCollectionFieldLayout {
                            field: "length".to_string(),
                            type_name: "i32".to_string(),
                            storage_type_name: "i32".to_string(),
                            element_count: None,
                        },
                    ],
                },
            ],
            structs: Vec::new(),
            opaque: Vec::new(),
        };

        let (source, register_lines) =
            build_aot_direct_storage_source(&layout).expect("build length storage source");
        assert!(
            source.contains("STASIS_EXPORT int32_t stasis_state_scalar__game__walls__length = 0;")
        );
        assert!(source
            .contains("STASIS_EXPORT int32_t stasis_state_scalar__game__walls__max_length = 2;"));
        assert!(source
            .contains("STASIS_EXPORT int32_t stasis_state_array__game__walls__length[2] = {0};"));
        assert!(register_lines.iter().any(|line| {
            line.contains("stasis_jit_register_global_i32_ptr")
                && line.contains(&crate::hash_global_path("game.walls.length").to_string())
        }));
        assert!(register_lines.iter().any(|line| {
            line.contains("stasis_jit_register_global_i32_array")
                && line.contains(&crate::hash_global_path("game.walls").to_string())
                && line.contains(&crate::hash_global_path("length").to_string())
        }));
    }

    #[test]
    fn packaged_runtime_bridge_preserves_unsigned_storage_widths() {
        let runtime_fields = vec![
            PackagedRuntimeField {
                name: "byte_value".to_string(),
                size: 1,
                field_type: "u8".to_string(),
                array_count: 1,
                initial_value: Some(serde_json::json!(255)),
                collection_path: None,
                collection_field: None,
                runtime_path: None,
            },
            PackagedRuntimeField {
                name: "word_values".to_string(),
                size: 4,
                field_type: "u16".to_string(),
                array_count: 2,
                initial_value: Some(serde_json::json!([1, 65535])),
                collection_path: Some("word_values".to_string()),
                collection_field: Some(String::new()),
                runtime_path: None,
            },
            PackagedRuntimeField {
                name: "wide_value".to_string(),
                size: 4,
                field_type: "u32".to_string(),
                array_count: 1,
                initial_value: Some(serde_json::json!(4294967295_u64)),
                collection_path: None,
                collection_field: None,
                runtime_path: None,
            },
        ];
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &runtime_fields,
            &[],
            &[],
            None,
            &[],
        )
        .expect("build runtime bridge");

        assert!(source.contains("typedef unsigned short uint16_t;"));
        assert!(source.contains("typedef unsigned int uint32_t;"));
        assert!(source.contains("uint8_t byte_value[1] = {255};"));
        assert!(source.contains("uint16_t word_values[2] = {1, 65535};"));
        assert!(source.contains("uint32_t wide_value = 4294967295;"));
        assert!(source.contains("stasis_jit_register_global_u8_array"));
        assert!(source.contains("stasis_jit_register_global_u16_array"));
        assert!(source.contains("(int32_t*)&wide_value"));
    }

    #[test]
    fn replay_snapshot_bridge_uses_explicit_kinds_and_canonical_little_endian_writes() {
        let snapshot = ProgramReplayStateSnapshot {
            schema: "stasis.replay_state_snapshot.v2".to_string(),
            abi_version: 2,
            support: "descriptor_only".to_string(),
            byte_order: "little_endian".to_string(),
            hash_scope: "simulation_after_tick".to_string(),
            required_bytes: 25,
            entries: vec![
                ProgramReplayStateEntry {
                    kind: "scalar".to_string(),
                    path: "flag".to_string(),
                    field: String::new(),
                    storage_type: "bool".to_string(),
                    offset: 0,
                    element_count: 1,
                    element_bytes: 1,
                },
                ProgramReplayStateEntry {
                    kind: "scalar".to_string(),
                    path: "scalar".to_string(),
                    field: String::new(),
                    storage_type: "i32".to_string(),
                    offset: 1,
                    element_count: 1,
                    element_bytes: 4,
                },
                ProgramReplayStateEntry {
                    kind: "scalar".to_string(),
                    path: "wide".to_string(),
                    field: String::new(),
                    storage_type: "f64".to_string(),
                    offset: 5,
                    element_count: 1,
                    element_bytes: 8,
                },
                ProgramReplayStateEntry {
                    kind: "collection".to_string(),
                    path: "named".to_string(),
                    field: "lane".to_string(),
                    storage_type: "f32".to_string(),
                    offset: 13,
                    element_count: 1,
                    element_bytes: 4,
                },
                ProgramReplayStateEntry {
                    kind: "collection".to_string(),
                    path: "primitive".to_string(),
                    field: String::new(),
                    storage_type: "i32".to_string(),
                    offset: 17,
                    element_count: 2,
                    element_bytes: 4,
                },
            ],
            unsupported_paths: Vec::new(),
            size_operation: "stasis_replay_state_snapshot_size".to_string(),
            write_operation: "stasis_replay_state_snapshot_write".to_string(),
            restore_operation: "stasis_replay_state_snapshot_restore".to_string(),
        };
        let source = build_engine_bundle_runtime_bridge_source_with_snapshot(
            &stasis_jit::AotTarget::Native,
            &[],
            &[],
            &[],
            None,
            &[],
            Some(&snapshot),
        )
        .expect("build replay snapshot bridge");

        assert!(source.contains("STASIS_EXPORT extern int32_t stasis_state_scalar__scalar;"));
        assert!(source.contains("STASIS_EXPORT extern int32_t stasis_state_array__primitive[];"));
        assert!(source.contains("stasis_state_array__primitive[0]"));
        assert!(source.contains("stasis_state_array__primitive[1]"));
        assert!(source.contains("STASIS_EXPORT extern float stasis_state_array__named__lane[];"));
        assert!(source.contains("STASIS_EXPORT extern int32_t stasis_state_scalar__flag;"));
        assert!(source.contains("STASIS_EXPORT extern double stasis_state_scalar__wide;"));
        assert!(source.contains("return 25;"));
        assert!(source.contains("return -required;"));
        assert!(source.contains("out[1] = (uint8_t)(bits32 & 0xffu);"));
        assert!(source.contains("out[4] = (uint8_t)((bits32 >> 24) & 0xffu);"));
        assert!(source
            .contains("out[0] = (uint8_t)(((int32_t)(stasis_state_scalar__flag) != 0) ? 1 : 0);"));
        assert!(source.contains("out[12] = (uint8_t)((bits64 >> 56) & 0xffull);"));
        assert!(source
            .contains("stasis_copy_bytes(&bits32, &(stasis_state_array__named__lane[0]), 4u);"));
        assert!(source.contains("stasis_copy_bytes(&bits64, &(stasis_state_scalar__wide), 8u);"));
        assert!(!source.contains("memcpy("));
        assert!(!source.contains("#include <string.h>"));
        assert!(source.contains(
            "STASIS_EXPORT int32_t stasis_replay_state_snapshot_restore(const uint8_t *input, int32_t bytes)"
        ));
        assert!(source.contains("if (bytes != required) return required == 0 ? -1 : -required;"));
        assert!(source
            .contains("stasis_copy_bytes(&(stasis_state_array__named__lane[0]), &bits32, 4u);"));
        assert!(source.contains("stasis_copy_bytes(&(stasis_state_scalar__wide), &bits64, 8u);"));
        let guard = source
            .find("if (out == (uint8_t *)0 || capacity < required) return -required;")
            .expect("capacity guard");
        let first_write = source.find("out[0] =").expect("first canonical write");
        assert!(
            guard < first_write,
            "short/null checks must precede all writes"
        );
        #[cfg(windows)]
        {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let temp_root = std::env::temp_dir().join(format!(
                "stasis-native-replay-bridge-syntax-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir_all(&temp_root).expect("create native replay bridge temp root");
            let source_path = temp_root.join("replay_bridge.c");
            let object_path = temp_root.join("replay_bridge.obj");
            fs::write(&source_path, &source).expect("write generated replay bridge C");
            let compiler = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bin/clang-cl.exe");
            let status = Command::new(&compiler)
                .arg("/nologo")
                .arg("/c")
                .arg("/TC")
                .arg(&source_path)
                .arg(format!("/Fo{}", object_path.display()))
                .status()
                .expect("run clang-cl for generated replay bridge");
            fs::remove_dir_all(&temp_root).ok();
            assert!(
                status.success(),
                "generated native replay bridge must compile as C with {}",
                compiler.display()
            );
        }

        let empty_snapshot = ProgramReplayStateSnapshot {
            schema: "stasis.replay_state_snapshot.v2".to_string(),
            abi_version: 2,
            support: "descriptor_only".to_string(),
            byte_order: "little_endian".to_string(),
            hash_scope: "simulation_after_tick".to_string(),
            required_bytes: 0,
            entries: Vec::new(),
            unsupported_paths: Vec::new(),
            size_operation: "stasis_replay_state_snapshot_size".to_string(),
            write_operation: "stasis_replay_state_snapshot_write".to_string(),
            restore_operation: "stasis_replay_state_snapshot_restore".to_string(),
        };
        let empty_source = build_engine_bundle_runtime_bridge_source_with_snapshot(
            &stasis_jit::AotTarget::Native,
            &[],
            &[],
            &[],
            None,
            &[],
            Some(&empty_snapshot),
        )
        .expect("build empty replay snapshot bridge");
        assert!(empty_source.contains("return 0;\n}\n"));
        assert!(empty_source.contains("(void)out;"));
        assert!(!empty_source.contains("stasis_copy_bytes("));
    }

    #[test]
    fn i32_only_replay_snapshot_emits_freestanding_copy_helper() {
        let snapshot = ProgramReplayStateSnapshot {
            schema: "stasis.replay_state_snapshot.v2".to_string(),
            abi_version: 2,
            support: "descriptor_only".to_string(),
            byte_order: "little_endian".to_string(),
            hash_scope: "simulation_after_tick".to_string(),
            required_bytes: 4,
            entries: vec![ProgramReplayStateEntry {
                kind: "scalar".to_string(),
                path: "signed".to_string(),
                field: String::new(),
                storage_type: "i32".to_string(),
                offset: 0,
                element_count: 1,
                element_bytes: 4,
            }],
            unsupported_paths: Vec::new(),
            size_operation: "stasis_replay_state_snapshot_size".to_string(),
            write_operation: "stasis_replay_state_snapshot_write".to_string(),
            restore_operation: "stasis_replay_state_snapshot_restore".to_string(),
        };
        let runtime_fields = [PackagedRuntimeField {
            name: "stasis_state_scalar__signed".to_string(),
            size: 4,
            field_type: "i32".to_string(),
            array_count: 1,
            initial_value: None,
            collection_path: None,
            collection_field: None,
            runtime_path: None,
        }];
        let source = build_engine_bundle_runtime_bridge_source_with_snapshot(
            &stasis_jit::AotTarget::Native,
            &runtime_fields,
            &[],
            &[],
            None,
            &[],
            Some(&snapshot),
        )
        .expect("build integer-only replay snapshot bridge");
        assert!(source.contains("STASIS_EXPORT extern int32_t stasis_state_scalar__signed;"));
        assert!(source.contains("static void stasis_copy_bytes("));
        assert!(source.contains("stasis_copy_bytes(&(stasis_state_scalar__signed), &bits32, 4u);"));
        let forward_declaration = source
            .find("STASIS_EXPORT extern int32_t stasis_state_scalar__signed;")
            .expect("exported integer storage forward declaration");
        let exported_definition = source
            .find("STASIS_EXPORT int32_t stasis_state_scalar__signed = 0;")
            .expect("matching exported integer storage definition");
        assert!(forward_declaration < exported_definition);
        assert!(!source.contains("memcpy("));
        assert!(!source.contains("#include <string.h>"));

        #[cfg(windows)]
        {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let temp_root = std::env::temp_dir().join(format!(
                "stasis-i32-replay-bridge-syntax-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir_all(&temp_root).expect("create integer replay bridge temp root");
            let source_path = temp_root.join("replay_bridge.c");
            let object_path = temp_root.join("replay_bridge.obj");
            fs::write(&source_path, &source).expect("write integer replay bridge C");
            let compiler = default_runtime_bridge_compiler(&stasis_jit::AotTarget::Native);
            let output = Command::new(compiler)
                .args(["/nologo", "/c", "/TC", "/Zl", "/GS-", "/X"])
                .arg(format!("/Fo{}", object_path.display()))
                .arg(&source_path)
                .output()
                .expect("run clang-cl for integer replay bridge");
            fs::remove_dir_all(&temp_root).ok();
            assert!(
                output.status.success(),
                "integer-only replay bridge must compile without standard headers: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn packaged_runtime_bridge_aliases_kind_qualified_bridge_storage() {
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &[],
            &[],
            &[],
            None,
            &[],
        )
        .expect("build runtime bridge");
        let aliases = [
            (
                aot_storage_symbol(AotStorageSymbolKind::Array, "host_i32", ""),
                "host_i32",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Array, "host_f32", ""),
                "host_f32",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_i32", ""),
                "gfx_cmd_i32",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_f32", ""),
                "gfx_cmd_f32",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_u8", ""),
                "gfx_cmd_u8",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_seq", ""),
                "host_req_seq",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_flags", ""),
                "host_req_flags",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_window_w_px", ""),
                "host_req_window_w_px",
            ),
            (
                aot_storage_symbol(AotStorageSymbolKind::Scalar, "host_req_window_h_px", ""),
                "host_req_window_h_px",
            ),
        ];
        for (qualified, legacy) in aliases {
            let expected = if cfg!(windows) {
                format!("#pragma comment(linker, \"/alternatename:{qualified}={legacy}\")")
            } else if cfg!(target_os = "macos") {
                format!("__asm__(\".globl _{qualified}\\n_{qualified} = _{legacy}\");")
            } else {
                format!(
                    "extern __typeof__({legacy}) {qualified} __attribute__((alias(\"{legacy}\")));"
                )
            };
            assert!(
                source.contains(&expected),
                "missing bridge alias {expected}"
            );
        }
    }

    #[test]
    fn packaged_runtime_bridge_aliases_ios_use_macho_assembly() {
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::ios_arm64_default(),
            &[],
            &[],
            &[],
            None,
            &[],
        )
        .expect("build iOS runtime bridge");
        let qualified = aot_storage_symbol(AotStorageSymbolKind::Array, "gfx_cmd_i32", "");
        assert!(source.contains(&format!(
            "__asm__(\".globl _{qualified}\\n_{qualified} = _gfx_cmd_i32\");"
        )));
        assert!(!source.contains("__attribute__((alias"));
    }

    #[test]
    fn package_support_capture_closes_nested_absolute_asset_graph() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_absolute_support_{stamp}"));
        let support = root.join("support");
        let external = root.join("external");
        fs::create_dir_all(&support).expect("create support directory");
        fs::create_dir_all(external.join("one")).expect("create first external directory");
        fs::create_dir_all(external.join("two")).expect("create second external directory");
        let first = external.join("one/shared.png");
        let second = external.join("two/shared.png");
        let table = external.join("values.csv");
        let metadata = external.join("metadata.JSON");
        fs::write(&first, b"first-image").expect("write first image");
        fs::write(&second, b"second-image").expect("write second image");
        fs::write(&table, b"id,value\n1,42\n").expect("write table");
        fs::write(
            &metadata,
            serde_json::to_vec(&serde_json::json!({"table": table})).expect("serialize metadata"),
        )
        .expect("write metadata");
        fs::write(
            support.join("config.json"),
            serde_json::to_vec(&serde_json::json!({
                "images": [first, second],
                "metadata": metadata,
            }))
            .expect("serialize support config"),
        )
        .expect("write support config");

        rewrite_packaged_json_asset_paths(&support, "main")
            .expect("capture absolute support graph");
        let config = fs::read_to_string(support.join("config.json")).expect("read config");
        assert!(!config.contains(&root.to_string_lossy().to_string()));
        let captured = fs::read_dir(support.join("assets/external"))
            .expect("read captured assets")
            .collect::<Result<Vec<_>, _>>()
            .expect("enumerate captured assets");
        assert_eq!(captured.len(), 4);
        assert_eq!(
            captured
                .iter()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with("-shared.png"))
                .count(),
            2,
            "content-derived names must preserve distinct same-basename assets"
        );

        fs::remove_dir_all(&external).expect("remove original absolute assets");
        rewrite_packaged_json_asset_paths(&support, "main")
            .expect("rewritten support must not read original paths");
        for entry in captured {
            if entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                let json = fs::read_to_string(entry.path()).expect("read captured nested JSON");
                assert!(!json.contains(&root.to_string_lossy().to_string()));
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn package_support_capture_rejects_missing_absolute_asset() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_missing_support_{stamp}"));
        fs::create_dir_all(&root).expect("create support directory");
        let missing = root.join("missing.png");
        fs::write(
            root.join("config.json"),
            serde_json::to_vec(&serde_json::json!({"image": missing}))
                .expect("serialize missing asset path"),
        )
        .expect("write support config");
        let error = rewrite_packaged_json_asset_paths(&root, "main")
            .expect_err("missing absolute package asset must fail capture");
        assert!(error.contains("referenced absolute package asset is unavailable"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_data_is_staged_and_embedded_in_aot_runtime_fields() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_project_data_{stamp}"));
        let project_dir = temp_root.join("project");
        let output_dir = temp_root.join("output");
        let entry_file = project_dir.join("src").join("main.stasis");
        let data_dir = project_dir.join("data");
        fs::create_dir_all(entry_file.parent().expect("entry parent")).expect("create src");
        fs::create_dir_all(&data_dir).expect("create data");
        fs::create_dir_all(&output_dir).expect("create output");
        fs::write(&entry_file, "function main(): i32 { return 0; }\n").expect("write entry");
        fs::write(
            data_dir.join("balance.json"),
            r#"{"hp":[70,110,85],"enabled":true}"#,
        )
        .expect("write data");
        fs::write(
            data_dir.join("balance.struct-meta.json"),
            r#"{
                "globalName":"balance",
                "fields":[
                    {"jsonPath":"hp","size":12,"type":"i32","arrayCount":3},
                    {"jsonPath":"enabled","size":1,"type":"bool","arrayCount":1}
                ]
            }"#,
        )
        .expect("write metadata");
        fs::write(
            data_dir.join("enemy.csv"),
            "cadence,damage\n90,9\n60,6\n120,18\n",
        )
        .expect("write CSV data");
        fs::write(
            data_dir.join("enemy.struct-meta.json"),
            r#"{
                "globalName":"enemy",
                "fields":[
                    {"jsonPath":"cadence","size":12,"type":"i32","arrayCount":3},
                    {"jsonPath":"damage","size":12,"type":"i32","arrayCount":3}
                ]
            }"#,
        )
        .expect("write CSV metadata");
        fs::write(data_dir.join("waves.csv"), "id,hp\n10,70\n20,110\n")
            .expect("write table CSV data");
        fs::write(
            data_dir.join("waves.struct-meta.json"),
            r#"{
                "globalName":"level",
                "csvTable":{
                    "rowsPath":"rows",
                    "rowCountPath":"row_count",
                    "capacity":4,
                    "keyColumns":["id"]
                },
                "fields":[
                    {"jsonPath":"rows.id","csvColumn":"id","size":16,"type":"i32","arrayCount":4},
                    {"jsonPath":"rows.hp","csvColumn":"hp","size":16,"type":"i32","arrayCount":4}
                ]
            }"#,
        )
        .expect("write table CSV metadata");

        let support = stage_entry_support_files(&project_dir, Some(&entry_file), &output_dir)
            .expect("stage project data");
        assert!(output_dir.join("data").join("balance.json").is_file());
        assert!(output_dir.join("data").join("enemy.csv").is_file());
        assert_eq!(support.runtime_fields.len(), 7);

        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &support.runtime_fields,
            &[],
            &[],
            None,
            &[],
        )
        .expect("build embedded data bridge");
        assert!(source.contains("int32_t balance__hp[3] = {70, 110, 85};"));
        assert!(source.contains("int32_t balance__enabled = 1;"));
        assert!(source.contains("int32_t enemy__cadence[3] = {90, 60, 120};"));
        assert!(source.contains("int32_t enemy__damage[3] = {9, 6, 18};"));
        assert!(source.contains("int32_t level__rows__id[4] = {10, 20, 0, 0};"));
        assert!(source.contains("int32_t level__rows__hp[4] = {70, 110, 0, 0};"));
        assert!(source.contains("int32_t level__row_count = 2;"));
        let rows_hash = crate::hash_global_path("level.rows");
        let id_hash = crate::hash_global_path("id");
        assert!(source.contains(&format!(
            "stasis_jit_register_global_i32_array({rows_hash}, {id_hash}, level__rows__id, 4);"
        )));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn engine_bundle_runtime_bridge_source_includes_android_entry_exports() {
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::android_arm64_default(),
            &[],
            &[
                "aot_fn_1".to_string(),
                "aot_fn_2".to_string(),
                "aot_fn_3".to_string(),
            ],
            &[
                PackagedFunctionAlias {
                    alias: "main",
                    target_symbol: "aot_fn_1".to_string(),
                    returns_i32: true,
                },
                PackagedFunctionAlias {
                    alias: "tick",
                    target_symbol: "aot_fn_2".to_string(),
                    returns_i32: true,
                },
                PackagedFunctionAlias {
                    alias: "render",
                    target_symbol: "aot_fn_3".to_string(),
                    returns_i32: true,
                },
            ],
            None,
            &[],
        )
        .expect("build android bridge source");

        assert!(!source.contains("StasisDirectStorageSlot"));
        assert!(source.contains("STASIS_EXPORT void stasis_init(int width, int height)"));
        assert!(source.contains("host_i32[14] = 4;"));
        assert!(source.contains("host_f32[56] = (float)width;"));
        assert!(source.contains("host_f32[57] = (float)height;"));
        assert!(source.contains("host_f32[50] = (float)width;"));
        assert!(source.contains("STASIS_EXPORT void stasis_tick(float dt)"));
        assert!(source.contains("host_i32[10] = host_i32[10] + 1;"));
        assert!(source.contains("STASIS_EXPORT void stasis_render(void)"));
        assert!(source.contains("STASIS_EXPORT void stasis_on_input(int type, int a, int b)"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_runtime_bridge_object_has_no_default_library_directives() {
        let runtime_fields = vec![PackagedRuntimeField {
            name: "balance__hp".to_string(),
            size: 12,
            field_type: "i32".to_string(),
            array_count: 3,
            initial_value: Some(serde_json::json!([70, 110, 85])),
            collection_path: None,
            collection_field: None,
            runtime_path: None,
        }];
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &runtime_fields,
            &[
                "aot_fn_1".to_string(),
                "aot_render".to_string(),
                "aot_reset".to_string(),
                "aot_finish".to_string(),
            ],
            &[PackagedFunctionAlias {
                alias: "main",
                target_symbol: "aot_fn_1".to_string(),
                returns_i32: true,
            }],
            Some(&PackagedRenderAlias {
                target_symbol: "aot_render".to_string(),
                returns_i32: true,
                reset_symbol: Some("aot_reset".to_string()),
                finish_symbol: Some("aot_finish".to_string()),
            }),
            &[],
        )
        .expect("build bridge source");
        assert!(source.contains("typedef signed int int32_t;"));
        assert!(source.contains("int32_t balance__hp[3] = {70, 110, 85};"));
        assert!(!source.starts_with("#include <stdint.h>"));
        assert!(!source.contains("#include <string.h>"));
        assert!(!source.contains("stasis_copy_bytes("));

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_bridge_directives_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source_path = temp_root.join("bridge.c");
        let object_path = temp_root.join("bridge.obj");
        fs::write(&source_path, source).expect("write bridge source");
        let compiler = default_runtime_bridge_compiler(&stasis_jit::AotTarget::Native);
        let output = Command::new(compiler)
            .args(["/nologo", "/c", "/O2", "/TC", "/Zl", "/GS-", "/X"])
            .arg(format!("/Fo{}", object_path.display()))
            .arg(&source_path)
            .output()
            .expect("compile bridge object");
        assert!(
            output.status.success(),
            "bridge compile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(&object_path).expect("read bridge object");
        let object = object::File::parse(&*bytes).expect("parse bridge object");
        if let Some(section) = object.section_by_name(".drectve") {
            let directives = String::from_utf8_lossy(section.data().expect("read directives"));
            assert!(!directives.to_ascii_uppercase().contains("DEFAULTLIB"));
        }
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn android_engine_bundle_skips_host_dynload_staticlib() {
        assert!(!should_link_stasis_dynload(
            &stasis_jit::AotTarget::android_arm64_default()
        ));
    }

    #[test]
    fn android_runtime_bridge_compiler_defaults_to_clang() {
        assert_eq!(
            default_runtime_bridge_compiler(&stasis_jit::AotTarget::android_arm64_default()),
            PathBuf::from("clang")
        );
    }

    #[test]
    fn compute_file_sha256_hex_matches_known_value() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_hash_known_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let payload = temp_root.join("payload.bin");
        fs::write(&payload, b"abc").expect("write payload");
        let hash = compute_file_sha256_hex(&payload).expect("hash should succeed");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_with_engine_entrypoints_builds_jit_engine_package_contract() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_jit_engine_package_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "function tick(): i32 { return 1; }\nfunction render(): i32 { return 2; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_100),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "brickout diagnostics: {:?}",
            result.diagnostics
        );
        let jit_overrides = result
            .jit_code_ptr_overrides
            .as_ref()
            .expect("jit overrides should be present in engine JIT mode");
        assert!(
            !jit_overrides.is_empty(),
            "jit overrides should include compiled function pointers"
        );
        assert!(
            jit_overrides.iter().all(|entry| entry.code_ptr != 0),
            "jit overrides should carry non-zero pointers"
        );
        let package = backend
            .last_jit_engine_package()
            .expect("jit engine package should be present");
        assert!(package.tick_code_ptr != 0);
        assert!(package.render_code_ptr != 0);
        assert!(package.on_code_swap_code_ptr.is_some());
        assert!(backend.last_aot_engine_bundle().is_none());
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_rejects_on_code_swap_with_non_void_return_type() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_jit_hook_sig_ret_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "function tick(): i32 { return 1; }\nfunction render(): i32 { return 2; }\nfunction on_code_swap(): i32 { return 0; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_101),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| diag.message.contains("invalid on_code_swap signature")),
            "expected hook signature diagnostic, got {:?}",
            result.diagnostics
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_rejects_on_code_swap_with_parameters() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_jit_hook_sig_params_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "function tick(): i32 { return 1; }\nfunction render(): i32 { return 2; }\nfunction on_code_swap(extra: i32): void { return; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_103),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diag| diag.message.contains("invalid on_code_swap signature")),
            "expected hook signature diagnostic, got {:?}",
            result.diagnostics
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn jit_dev_brickout_v1_builds_engine_package_with_render_pointer() {
        let _global_guard = crate::jit_test_support::lock();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("samples")
            .join("brickout_revenge")
            .join("brickout_revenge_v1.stasis");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_120),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "brickout diagnostics: {:?}",
            result.diagnostics
        );
        let package = backend
            .last_jit_engine_package()
            .expect("jit engine package should be present");
        assert!(package.tick_code_ptr != 0);
        assert!(package.render_code_ptr != 0);
        assert!(
            package.symbol_code_ptrs.contains_key("render"),
            "expected render symbol in JIT engine package"
        );
    }

    #[test]
    fn aot_brickout_revenge_v1_compiles_full_engine_bundle() {
        let _global_guard = crate::jit_test_support::lock();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("samples")
            .join("brickout_revenge")
            .join("brickout_revenge_v1.stasis");
        assert!(
            source.exists(),
            "expected Brickout sample at {}",
            source.display()
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_brickout_bundle_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let artifact_root = temp_root.join("aot_artifacts");

        let mut backend =
            IncrementalCompilerBackend::with_aot_config(AotCompileConfig::default(), artifact_root);
        let result = backend.compile(CompileRequest::new(
            RequestId(9_201),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "expected Brickout AOT compile success, got diagnostics: {:?}",
            result.diagnostics
        );

        let bundle = backend
            .last_aot_engine_bundle()
            .expect("AOT engine bundle should be present after successful compile");
        assert!(
            bundle.manifest_path.exists(),
            "engine bundle manifest should exist at {}",
            bundle.manifest_path.display()
        );

        let manifest = backend
            .read_engine_bundle_manifest(&bundle.manifest_path)
            .expect("read engine bundle manifest");

        for required in ["tick", "render", "on_code_swap"] {
            assert!(
                bundle.object_paths_by_function.contains_key(required),
                "missing required entrypoint '{required}' in AOT bundle object map"
            );
            let object_path = bundle
                .object_paths_by_function
                .get(required)
                .expect("checked contains_key");
            assert!(
                object_path.exists(),
                "AOT bundle object for '{required}' should exist at {}",
                object_path.display()
            );
            assert!(
                manifest.functions.iter().any(|row| row.name == required),
                "engine bundle manifest missing function row for '{required}'"
            );
        }

        assert!(
            manifest
                .string_literals
                .as_ref()
                .is_some_and(|values| !values.is_empty()),
            "expected Brickout engine bundle manifest to include string_literals"
        );
        assert!(
            manifest
                .collection_max_lengths
                .as_ref()
                .is_some_and(|values| !values.is_empty()),
            "expected Brickout engine bundle manifest to include collection_max_lengths"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_brickout_revenge_v1_engine_bundle_executes_two_ticks() {
        let _global_guard = crate::jit_test_support::lock();
        fn hash_global_path(path: &str) -> i32 {
            let mut hash: u32 = 2_166_136_261;
            for byte in path.bytes() {
                hash ^= u32::from(byte);
                hash = hash.wrapping_mul(16_777_619);
            }
            hash as i32
        }

        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("samples")
            .join("brickout_revenge")
            .join("brickout_revenge_v1.stasis");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_brickout_exec_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            temp_root.join("aot_artifacts"),
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(9_202),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "Brickout AOT diagnostics: {:?}",
            result.diagnostics
        );

        let bundle = backend
            .last_aot_engine_bundle()
            .expect("compiled Brickout engine bundle");
        let manifest = backend
            .read_engine_bundle_manifest(&bundle.manifest_path)
            .expect("read Brickout bundle manifest");
        let symbol = |name: &str| {
            manifest
                .functions
                .iter()
                .find(|row| row.name == name)
                .map(|row| row.symbol.clone())
                .unwrap_or_else(|| panic!("manifest should include {name}"))
        };
        let main_symbol = symbol("main");
        let tick_symbol = symbol("tick");
        let state_layout = backend
            .last_program_snapshot
            .as_ref()
            .map(ProgramSnapshot::state_layout)
            .expect("Brickout AOT snapshot");
        let runtime_fields =
            merge_runtime_fields(state_layout, &[]).expect("Brickout runtime fields");
        let function_symbols = manifest
            .functions
            .iter()
            .map(|row| row.symbol.clone())
            .collect::<Vec<_>>();
        let function_aliases = vec![
            PackagedFunctionAlias {
                alias: "main",
                target_symbol: main_symbol.clone(),
                returns_i32: true,
            },
            PackagedFunctionAlias {
                alias: "tick",
                target_symbol: tick_symbol.clone(),
                returns_i32: true,
            },
        ];
        let bridge_object = emit_engine_bundle_runtime_bridge_object(
            &backend,
            &runtime_fields,
            &function_symbols,
            &function_aliases,
            None,
            manifest.string_literals.as_deref().unwrap_or_default(),
            None,
        )
        .expect("compile Brickout runtime bridge");

        let mut link_config = AotLinkConfig::default();
        link_config
            .runtime_lib_paths
            .push(ensure_stasis_dynload_link_library().expect("stasis_dynload link library"));
        if let Some(wrapper) = ensure_rust_lld_link_wrapper(&temp_root) {
            link_config.linker_path = Some(wrapper);
        }
        let linked = temp_root.join("brickout_aot_bundle.dll");
        let export_symbols = vec![
            main_symbol.clone(),
            tick_symbol.clone(),
            "stasis_jit_global_i32_array_store".to_string(),
            "stasis_jit_global_f32_array_store".to_string(),
        ];
        let mut object_paths = bundle.object_paths().cloned().collect::<Vec<_>>();
        object_paths.push(bridge_object);
        link_objects_to_dynamic_library(&object_paths, &linked, &export_symbols, &link_config)
            .expect("link Brickout engine bundle");
        sign_output_artifact_if_configured(&linked).expect("sign Brickout engine bundle");

        let library = stasis_dynload::Library::load(&linked).expect("load Brickout engine bundle");
        let main_ptr = library
            .symbol_address(&main_symbol)
            .expect("resolve Brickout main");
        let tick_ptr = library
            .symbol_address(&tick_symbol)
            .expect("resolve Brickout tick");
        let store_i32 = library
            .symbol_address("stasis_jit_global_i32_array_store")
            .expect("resolve host i32 store");
        let store_f32 = library
            .symbol_address("stasis_jit_global_f32_array_store")
            .expect("resolve host f32 store");
        let bind_runtime = library
            .symbol_address("stasis_aot_bind_runtime_globals")
            .expect("resolve runtime-global binding entrypoint");
        stasis_dynload::invoke_noarg_void(bind_runtime).expect("bind Brickout runtime globals");
        let host_i32 = hash_global_path("host_i32");
        let host_f32 = hash_global_path("host_f32");
        let store = |index: i32, value: i32| {
            stasis_dynload::invoke_i32_i32_i32_i32_to_void(store_i32, host_i32, 0, index, value)
                .expect("store Brickout host i32");
        };
        let store_float = |index: i32, value: f32| {
            stasis_dynload::invoke_i32_i32_i32_f32_to_void(store_f32, host_f32, 0, index, value)
                .expect("store Brickout host f32");
        };

        let start_ms = 12_345;
        store(0, start_ms);
        store_float(50, 360.0);
        store_float(51, 720.0);
        store_float(52, 0.0);
        store_float(53, 0.0);
        store_float(54, 360.0);
        store_float(55, 720.0);
        store(7, 0);
        store(8, 0);
        store(9, 0);
        store(10, 0);
        store(11, 1);
        store(12, 360);
        store(13, 720);
        store(19, start_ms * 1_000);

        assert_eq!(
            stasis_dynload::invoke_noarg_i32(main_ptr).expect("execute Brickout main"),
            0
        );
        store(11, 0);
        for tick_index in 0..2 {
            let time_ms = start_ms + (tick_index + 1) * 16;
            store(0, time_ms);
            store(10, tick_index);
            store(19, time_ms * 1_000);
            assert_eq!(
                stasis_dynload::invoke_noarg_i32(tick_ptr).expect("execute Brickout tick"),
                0
            );
        }

        drop(library);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_engine_mode_rebuilds_one_complete_generation_between_compiles() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_jit_engine_reuse_artifacts_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "function tick(): i32 { return 1; }\nfunction render(): i32 { return 2; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let first = backend.compile(CompileRequest::new(
            RequestId(9_111),
            vec![source.clone()],
            TargetMode::JitDev,
        ));
        assert_eq!(first.status, CompileStatus::Success);
        let revision_before = backend
            .jit_generation_source_revision()
            .expect("generation metadata after first compile");
        let tick_slot_before = backend
            .jit_artifact_slot_for_function_name("tick")
            .expect("tick slot after first compile");
        let render_slot_before = backend
            .jit_artifact_slot_for_function_name("render")
            .expect("render slot after first compile");

        fs::write(
            &source,
            "function tick(): i32 { return 3; }\nfunction render(): i32 { return 2; }\n",
        )
        .expect("rewrite source");
        let second = backend.compile(CompileRequest::new(
            RequestId(9_112),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(second.status, CompileStatus::Success);
        let revision_after = backend
            .jit_generation_source_revision()
            .expect("generation metadata after second compile");
        let tick_slot_after = backend
            .jit_artifact_slot_for_function_name("tick")
            .expect("tick slot after second compile");
        let render_slot_after = backend
            .jit_artifact_slot_for_function_name("render")
            .expect("render slot after second compile");

        assert_eq!(
            tick_slot_after, tick_slot_before,
            "stable generation-local function order should preserve the tick slot"
        );
        assert_eq!(
            render_slot_after, render_slot_before,
            "stable generation-local function order should preserve the render slot"
        );
        assert_ne!(
            revision_after, revision_before,
            "the body edit should publish a distinct complete generation"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_layout_hash_ignores_function_body_only_edits() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_layout_hash_body_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("main.stasis");
        fs::write(
            &source,
            "global score: i32;\nfunction main(): i32 { return score + 1; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let first = backend.compile(CompileRequest::new(
            RequestId(9_113),
            vec![source.clone()],
            TargetMode::JitDev,
        ));
        assert_eq!(first.status, CompileStatus::Success);

        fs::write(
            &source,
            "global score: i32;\nfunction main(): i32 { return score + 2; }\n",
        )
        .expect("rewrite source");
        let second = backend.compile(CompileRequest::new(
            RequestId(9_114),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(second.status, CompileStatus::Success);
        assert_eq!(
            first.layout_hash, second.layout_hash,
            "function body changes must not create a parallel layout identity"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_non_engine_source_exposes_canonical_jit_code_ptr_overrides() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_jit_non_engine_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("game_logic.stasis");
        fs::write(
            &source,
            "function damage(enemy: i32, amount: i32): i32 { return enemy - amount; }\nfunction main(): i32 { let enemy: i32 = 10; if (enemy > 4) { return enemy.damage(3); } return 0; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_102),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Success);
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for non-engine rust-native jit compile"
        );
        let patch_set = result
            .fn_patch_set
            .as_ref()
            .expect("patch set should be present");
        assert_eq!(
            patch_set.functions.len(),
            2,
            "every reachable canonical function identity should be patched"
        );
        let overrides = result
            .jit_code_ptr_overrides
            .as_ref()
            .expect("jit code pointer overrides should be present for non-engine jit path");
        assert_eq!(
            overrides.len(),
            2,
            "canonical pointer overrides must not collapse internal functions by name"
        );
        assert!(
            overrides.iter().all(|entry| entry.code_ptr != 0),
            "non-engine jit overrides should use non-zero executable code pointers"
        );
        assert!(backend.last_jit_engine_package().is_none());
        assert!(backend.last_aot_engine_bundle().is_none());
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_non_engine_accepts_for_loop_decrement_step() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_jit_non_engine_for_sub_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("game_logic.stasis");
        fs::write(
            &source,
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 5; i > 0; i -= 2) { sum += i; } return sum; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_104),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Success);
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for for-loop decrement-step compile"
        );
        let overrides = result
            .jit_code_ptr_overrides
            .as_ref()
            .expect("jit code pointer overrides should be present");
        assert_eq!(overrides.len(), 1, "expected one compiled function");
        assert!(
            overrides.iter().all(|entry| entry.code_ptr != 0),
            "jit overrides should carry non-zero pointers"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_non_engine_accepts_if_else_if_else_shape() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_jit_non_engine_if_else_if_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("game_logic.stasis");
        fs::write(
            &source,
            "function main(): i32 { let value: i32 = 2; if (value == 0) { return 1; } else if (value == 2) { return 5; } else { return 9; } }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_105),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Success);
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for if/else-if/else compile"
        );
        let overrides = result
            .jit_code_ptr_overrides
            .as_ref()
            .expect("jit code pointer overrides should be present");
        assert_eq!(overrides.len(), 1, "expected one compiled function");
        assert!(
            overrides.iter().all(|entry| entry.code_ptr != 0),
            "jit overrides should carry non-zero pointers"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_non_engine_accepts_logical_condition_shape() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_jit_non_engine_logical_condition_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("game_logic.stasis");
        fs::write(
            &source,
            "function main(): i32 { let value: i32 = 2; if ((value > 1 && value < 4) || !(value == 2)) { return 11; } return 0; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_106),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Success);
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for logical condition compile"
        );
        let overrides = result
            .jit_code_ptr_overrides
            .as_ref()
            .expect("jit code pointer overrides should be present");
        assert_eq!(overrides.len(), 1, "expected one compiled function");
        assert!(
            overrides.iter().all(|entry| entry.code_ptr != 0),
            "jit overrides should carry non-zero pointers"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_non_engine_accepts_for_loop_logical_condition_shape() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_jit_non_engine_for_logical_condition_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("game_logic.stasis");
        fs::write(
            &source,
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; (i < 5) && !(i == 3); i += 1) { sum += i; } return sum; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_107),
            vec![source],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Success);
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for for-loop logical condition compile"
        );
        let overrides = result
            .jit_code_ptr_overrides
            .as_ref()
            .expect("jit code pointer overrides should be present");
        assert_eq!(overrides.len(), 1, "expected one compiled function");
        assert!(
            overrides.iter().all(|entry| entry.code_ptr != 0),
            "jit overrides should carry non-zero pointers"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_non_engine_rejects_duplicate_function_names_without_legacy_fallback() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_jit_non_engine_dup_names_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let file_a = temp_root.join("a.stasis");
        let file_b = temp_root.join("b.stasis");
        fs::write(&file_a, "function main(): i32 { return 1; }\n").expect("write file a");
        fs::write(&file_b, "function main(): i32 { return 2; }\n").expect("write file b");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_103),
            vec![file_a, file_b],
            TargetMode::JitDev,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("host ABI alias 'main' requires exactly one canonical identity")),
            "expected host ABI alias ambiguity diagnostic"
        );
        assert!(result.jit_code_ptr_overrides.is_none());
        assert!(backend.last_jit_engine_package().is_none());
        assert!(backend.last_aot_engine_bundle().is_none());
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_prod_with_engine_entrypoints_builds_aot_engine_bundle_contract() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_engine_bundle_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("engine.stasis");
        fs::write(
            &source,
            "function tick(): i32 { return 1; }\nfunction render(): i32 { return 2; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(9_101),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);
        let bundle = backend
            .last_aot_engine_bundle()
            .expect("aot engine bundle should be present");
        assert!(bundle.manifest_path.exists());
        assert!(bundle.object_paths_by_function.contains_key("tick"));
        assert!(bundle.object_paths_by_function.contains_key("render"));
        assert_eq!(
            result.aot_linked_image_path,
            Some(bundle.manifest_path.clone())
        );
        assert!(result.aot_linked_image_size_bytes.is_some());
        assert!(result.aot_linked_image_sha256.is_some());
        assert!(backend.last_jit_engine_package().is_none());
        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn aot_packaged_render_bridge_executes_host_owned_generations() {
        let _global_guard = crate::jit_test_support::lock();
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("canonical repository root");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = repository_root
            .join(".stasis_cache")
            .join(format!("aot_packaged_render_lifecycle_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create AOT lifecycle fixture directory");
        let source = temp_root.join("render_lifecycle_probe.stasis");
        fs::write(
            &source,
            r#"import "../../src/stdlib/graphics.stasis";
global render_calls: i32;
function main(): i32 { return 0; }
function tick(): i32 { return 0; }
function render(): i32 {
    render_calls = render_calls + 1;
    clear(0.1, 0.2, 0.3, 1.0);
    fill_rect(2.0, 3.0, 4.0, 5.0, 0.4, 0.5, 0.6, 1.0);
    return 0;
}
"#,
        )
        .expect("write AOT lifecycle fixture");

        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(AotCompileConfig::default(), artifact_root);
        let result = backend.compile(CompileRequest::new(
            RequestId(9_605),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "AOT lifecycle fixture diagnostics: {:?}",
            result.diagnostics
        );

        let bundle = backend
            .last_aot_engine_bundle()
            .expect("AOT lifecycle engine bundle")
            .clone();
        let manifest = backend
            .read_engine_bundle_manifest(&bundle.manifest_path)
            .expect("read AOT lifecycle manifest");
        assert_eq!(manifest.render_construction_lifecycle_version, 1);
        let render_row = manifest
            .functions
            .iter()
            .find(|row| is_zero_argument_manifest_function(row, "render"))
            .expect("zero-argument render row");
        let render_alias = packaged_render_alias(&manifest, render_row)
            .expect("AOT lifecycle helper pair should be valid");
        assert!(render_alias.reset_symbol.is_some());
        assert!(render_alias.finish_symbol.is_some());
        assert_eq!(render_alias.returns_i32, true);
        for helper in ["gfx_cmd_construction_reset", "gfx_cmd_construction_finish"] {
            assert_eq!(
                manifest
                    .functions
                    .iter()
                    .filter(|row| row.name == helper)
                    .count(),
                1,
                "AOT lifecycle manifest should contain exactly one {helper} helper"
            );
        }

        let snapshot = backend
            .last_program_snapshot
            .as_ref()
            .expect("AOT lifecycle program snapshot");
        let runtime_fields = merge_runtime_fields(snapshot.state_layout(), &[])
            .expect("derive AOT lifecycle runtime fields");
        assert!(runtime_fields.iter().any(|field| {
            field.name
                == aot_storage_symbol(
                    AotStorageSymbolKind::Scalar,
                    "gfx_sprite_writer_frame_generation",
                    "",
                )
        }));
        let function_symbols = manifest
            .functions
            .iter()
            .map(|row| row.symbol.clone())
            .collect::<Vec<_>>();
        let aliases = ["main", "tick"]
            .into_iter()
            .map(|name| PackagedFunctionAlias {
                alias: name,
                target_symbol: resolve_engine_bundle_symbol(&manifest, name)
                    .expect("resolve AOT lifecycle entrypoint"),
                returns_i32: true,
            })
            .collect::<Vec<_>>();
        let bridge_object = emit_engine_bundle_runtime_bridge_object(
            &backend,
            &runtime_fields,
            &function_symbols,
            &aliases,
            Some(&render_alias),
            manifest.string_literals.as_deref().unwrap_or_default(),
            None,
        )
        .expect("compile production AOT lifecycle bridge");
        assert!(bridge_object.exists());
        let bridge_source = fs::read_to_string(
            backend
                .aot_artifact_root
                .join("engine_bundle_runtime_bridge.c"),
        )
        .expect("read emitted AOT lifecycle bridge source");
        let render_wrapper = bridge_source
            .lines()
            .find(|line| line.starts_with("STASIS_EXPORT int32_t render(void)"))
            .expect("render lifecycle wrapper");
        assert!(render_wrapper.contains(
            render_alias
                .reset_symbol
                .as_deref()
                .expect("render lifecycle reset symbol")
        ));
        assert!(render_wrapper.contains(
            render_alias
                .finish_symbol
                .as_deref()
                .expect("render lifecycle finish symbol")
        ));

        let mut objects = bundle.object_paths().cloned().collect::<Vec<_>>();
        objects.push(bridge_object);
        let linked = temp_root.join("aot_packaged_render_lifecycle.so");
        let dynload = ensure_stasis_dynload_link_library().expect("stasis dynload link library");
        link_objects_to_dynamic_library(
            &objects,
            &linked,
            &[
                "render".to_string(),
                "stasis_aot_bind_runtime_globals".to_string(),
                "stasis_jit_global_i32_load".to_string(),
                "stasis_jit_global_i32_array_load".to_string(),
            ],
            &AotLinkConfig {
                linker_path: None,
                runtime_lib_paths: vec![dynload],
                target: stasis_jit::AotTarget::default(),
            },
        )
        .expect("link production AOT lifecycle bridge");

        let library = DynamicLibrary::load(&linked).expect("load production AOT lifecycle bridge");
        let bind = library
            .symbol_address("stasis_aot_bind_runtime_globals")
            .expect("resolve AOT lifecycle global binding");
        stasis_dynload::invoke_noarg_void(bind).expect("bind AOT lifecycle globals");
        let render = library
            .symbol_address("render")
            .expect("resolve production render alias");
        let load_i32 = library
            .symbol_address("stasis_jit_global_i32_load")
            .expect("resolve scalar global accessor");
        let load_i32_array = library
            .symbol_address("stasis_jit_global_i32_array_load")
            .expect("resolve array global accessor");
        let render_calls_hash = stasis_dynload::global_path_hash("render_calls");
        let generation_hash =
            stasis_dynload::global_path_hash("gfx_sprite_writer_frame_generation");
        let gfx_cmd_i32_hash = stasis_dynload::global_path_hash("gfx_cmd_i32");
        let read_scalar =
            |path_hash| invoke_i32_to_i32(load_i32, path_hash).expect("read scalar runtime global");
        let read_gfx = |index| {
            invoke_i32_i32_i32_to_i32(load_i32_array, gfx_cmd_i32_hash, 0, index)
                .expect("read command buffer global")
        };

        assert_eq!(invoke_noarg_i32(render), Ok(0));
        assert_eq!(read_scalar(render_calls_hash), 1);
        assert_eq!(
            read_scalar(generation_hash),
            1,
            "one wrapper reset starts generation one"
        );
        assert_eq!(read_gfx(2), 3, "clear plus present publishes frame");
        assert_eq!(read_gfx(24), 1, "published frame contains one rectangle");

        assert_eq!(invoke_noarg_i32(render), Ok(0));
        assert_eq!(read_scalar(render_calls_hash), 2);
        assert_eq!(
            read_scalar(generation_hash),
            2,
            "each host-owned render invocation starts one construction generation"
        );
        assert_eq!(
            read_gfx(2),
            3,
            "host finish publishes clear and present flags"
        );
        assert_eq!(read_gfx(24), 1, "published frame contains one rectangle");

        drop(library);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_rejects_unresolved_direct_call_target() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_unresolved_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return callee(); }\n").expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(131),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("cannot resolve call")
                    && diagnostic.message.contains("callee")
            }),
            "expected unresolved call diagnostic, got: {:?}",
            result.diagnostics
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_direct_call_target_without_fallback() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_known_host_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function @extern(\"stasis_jit_global_i32_load\") host_load(path_hash: i32): i32;\nfunction main(): i32 { return host_load(123) + 10; }\n",
        )
        .expect("write source");

        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            artifact_root.clone(),
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(136),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let _: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_writes_manifest_with_artifacts_on_success() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_manifest_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return 0; }\n").expect("write source");

        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            artifact_root.clone(),
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(99),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        assert!(manifest_path.exists());
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert_eq!(manifest.request_id, 99);
        assert!(!manifest.artifact_paths.is_empty());
        assert!(manifest.linked_image_path.is_none());
        assert!(manifest.linked_image_sha256.is_none());

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_emits_hook_fn_symbol_mapping_and_patch_coverage() {
        let _global_guard = crate::jit_test_support::lock();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_symbol_map_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            temp_root.join("aot_artifacts"),
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(121),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let symbols = result
            .aot_function_symbols
            .as_ref()
            .expect("AotProd compile should emit function symbols");
        let patch_set = result
            .fn_patch_set
            .as_ref()
            .expect("successful compile should include patch set");
        assert_eq!(symbols.len(), patch_set.functions.len());
        assert!(symbols
            .iter()
            .all(|entry| entry.symbol.starts_with("aot_fn_")));

        let hook_fn_id = result
            .hook_fn_id
            .expect("hook function id should be populated for on_code_swap");
        assert!(symbols.iter().any(|entry| entry.fn_id == hook_fn_id));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    fn find_lld_link() -> Option<PathBuf> {
        let candidates = [
            r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\lld-link.exe",
            r"C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\lld-link.exe",
        ];
        candidates
            .iter()
            .map(PathBuf::from)
            .find(|path| path.exists())
    }

    #[cfg(windows)]
    #[test]
    fn aot_compile_with_real_linker_exports_emitted_symbols_when_available() {
        let _global_guard = crate::jit_test_support::lock();
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_real_exports_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            AotCompileConfig::default(),
            AotLinkConfig {
                linker_path: Some(linker_path),
                runtime_lib_paths: vec![],
                target: stasis_jit::AotTarget::default(),
            },
            temp_root.join("aot_artifacts"),
            true,
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(122),
            vec![source],
            TargetMode::AotProd,
        ));
        if result.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }

        let linked = result
            .aot_linked_image_path
            .as_ref()
            .expect("linked image should be produced");
        let bytes = fs::read(linked).expect("read linked image");
        let file = object::File::parse(&*bytes).expect("parse linked image");
        let exports: Vec<String> = file
            .exports()
            .expect("exports should parse")
            .into_iter()
            .map(|entry| String::from_utf8_lossy(entry.name()).to_string())
            .collect();
        let symbols = result
            .aot_function_symbols
            .as_ref()
            .expect("AOT symbols should be populated");
        for expected in symbols {
            assert!(exports.iter().any(|name| name == &expected.symbol));
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_emitted_symbol_executes_direct_call_semantics_if_real_link_available() {
        let _global_guard = crate::jit_test_support::lock();
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_direct_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function callee(): i32 { return 7; }\nfunction main(): i32 { return callee(); }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            AotCompileConfig::default(),
            AotLinkConfig {
                linker_path: Some(linker_path),
                runtime_lib_paths: vec![],
                target: stasis_jit::AotTarget::default(),
            },
            temp_root.join("aot_artifacts"),
            true,
        );
        let compiled = backend.compile(CompileRequest::new(
            RequestId(129),
            vec![source.clone()],
            TargetMode::AotProd,
        ));
        if compiled.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }
        let Some(linked_path) = compiled.aot_linked_image_path.as_ref() else {
            fs::remove_dir_all(&temp_root).ok();
            return;
        };
        let function_symbols = compiled
            .aot_function_symbols
            .as_ref()
            .expect("successful AOT link should include function symbols");
        let compiled_symbol_for = |name: &str| {
            let fn_id = FnId(
                backend
                    .last_program_snapshot
                    .as_ref()
                    .and_then(|snapshot| {
                        snapshot
                            .functions()
                            .iter()
                            .find(|function| function.name == name)
                    })
                    .unwrap_or_else(|| panic!("missing compiler function identity for {name}"))
                    .id,
            );
            function_symbols
                .iter()
                .find(|entry| entry.fn_id == fn_id)
                .unwrap_or_else(|| panic!("missing AOT symbol for {name}"))
                .symbol
                .clone()
        };
        let expected_main_symbol = compiled_symbol_for("main");
        let expected_callee_symbol = compiled_symbol_for("callee");

        let library = DynamicLibrary::load(linked_path).expect("load linked image");
        let main_ptr = library
            .symbol_address(&expected_main_symbol)
            .expect("resolve main symbol");
        let callee_ptr = library
            .symbol_address(&expected_callee_symbol)
            .expect("resolve callee symbol");
        let main_value = invoke_noarg_u64(main_ptr).expect("invoke main");
        let callee_value = invoke_noarg_u64(callee_ptr).expect("invoke callee");
        assert_eq!(main_value, callee_value);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_bundle_executes_direct_global_storage_if_real_link_available() {
        let _global_guard = crate::jit_test_support::lock();
        let Some(linker_path) = find_lld_link() else {
            return;
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_direct_storage_{stamp}"));
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project root");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "struct Enemy { hp: i32; speed: f32; precise: f64; }\nglobal count: i32;\nglobal ratio: f32;\nglobal precise: f64;\nglobal values: i32[2];\nglobal float_values: f32[2];\nglobal double_values: f64[2];\nglobal bytes: u8[3];\nglobal enemies: Enemy[1];\nglobal label: ascii[4];\nfunction main(): i32 { count = 7; ratio = 1.5; precise = 2.5; values[1] = 11; float_values[0] = 3.5; double_values[1] = 4.5; bytes[2] = 250; foreach (let byte in bytes) { byte += 1; } enemies[0].hp = 13; enemies[0].speed = 6.5; enemies[0].precise = 7.5; label[0] = 65; if (ratio < 1.4 || precise < 2.4 || float_values[0] < 3.4 || double_values[1] < 4.4 || enemies[0].speed < 6.4 || enemies[0].precise < 7.4) { return -1; } return count + values[1] + bytes[2] + enemies[0].hp + label[0] + label.max_length; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\n",
        )
        .expect("write source");

        let artifact_root = temp_root.join("artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(AotCompileConfig::default(), artifact_root);
        let result = backend.compile(CompileRequest::new(
            RequestId(17_200),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "AOT direct storage compile failed: {:?}",
            result.diagnostics
        );
        let bundle = backend
            .last_aot_engine_bundle()
            .expect("AOT engine bundle")
            .clone();
        let manifest = backend
            .read_engine_bundle_manifest(&bundle.manifest_path)
            .expect("read manifest");
        let runtime_fields = merge_runtime_fields(
            backend
                .last_program_snapshot
                .as_ref()
                .expect("program snapshot")
                .state_layout(),
            &[],
        )
        .expect("derive AOT storage fields");
        let aliases = ["main", "tick", "render"]
            .into_iter()
            .map(|name| PackagedFunctionAlias {
                alias: name,
                target_symbol: resolve_engine_bundle_symbol(&manifest, name)
                    .expect("resolve entrypoint"),
                returns_i32: true,
            })
            .collect::<Vec<_>>();
        let function_symbols = manifest
            .functions
            .iter()
            .map(|row| row.symbol.clone())
            .collect::<Vec<_>>();
        let bridge = emit_engine_bundle_runtime_bridge_object(
            &backend,
            &runtime_fields,
            &function_symbols,
            &aliases,
            None,
            &[],
            None,
        )
        .expect("compile direct storage bridge");
        let mut objects = bundle.object_paths().cloned().collect::<Vec<_>>();
        objects.push(bridge);
        let linked = temp_root.join("direct_storage.dll");
        let dynload = ensure_stasis_dynload_link_library().expect("stasis dynload link library");
        stasis_jit::link_objects_to_dynamic_library(
            &objects,
            &linked,
            &[
                "main".to_string(),
                "stasis_aot_bind_runtime_globals".to_string(),
            ],
            &AotLinkConfig {
                linker_path: Some(linker_path),
                runtime_lib_paths: vec![dynload.clone()],
                target: stasis_jit::AotTarget::default(),
            },
        )
        .expect("link direct storage AOT bundle");
        stage_stasis_dynload_runtime(&dynload, &linked).expect("stage stasis dynload runtime");

        let library = DynamicLibrary::load(&linked).expect("load direct storage AOT bundle");
        let bind = library
            .symbol_address("stasis_aot_bind_runtime_globals")
            .expect("resolve runtime binding");
        stasis_dynload::invoke_noarg_void(bind).expect("bind runtime globals");
        let main = library.symbol_address("main").expect("resolve main");
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(main).expect("execute AOT main"),
            351
        );
        // Runtime bindings intentionally remain valid until the test process exits.
        std::mem::forget(library);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn bounded_performance_sample_links_and_executes_aot_if_real_link_available() {
        let _global_guard = crate::jit_test_support::lock();
        let Some(linker_path) = find_lld_link() else {
            return;
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.stasis_cache/tmp")
            .join(format!("stasis_bounded_performance_aot_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create stable AOT test root");
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/bounded_performance/src/main.stasis");
        let mut backend = IncrementalCompilerBackend::with_aot_config(
            AotCompileConfig::default(),
            temp_root.join("artifacts"),
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(17_201),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(
            result.status,
            CompileStatus::Success,
            "{:?}",
            result.diagnostics
        );
        let bundle = backend
            .last_aot_engine_bundle()
            .expect("AOT engine bundle")
            .clone();
        let manifest = backend
            .read_engine_bundle_manifest(&bundle.manifest_path)
            .expect("read manifest");
        let runtime_fields = merge_runtime_fields(
            backend
                .last_program_snapshot
                .as_ref()
                .expect("program snapshot")
                .state_layout(),
            &[],
        )
        .expect("derive AOT storage fields");
        let aliases = ["main", "tick", "render"]
            .into_iter()
            .map(|name| PackagedFunctionAlias {
                alias: name,
                target_symbol: resolve_engine_bundle_symbol(&manifest, name)
                    .expect("resolve entrypoint"),
                returns_i32: true,
            })
            .collect::<Vec<_>>();
        let function_symbols = manifest
            .functions
            .iter()
            .map(|row| row.symbol.clone())
            .collect::<Vec<_>>();
        let bridge = emit_engine_bundle_runtime_bridge_object(
            &backend,
            &runtime_fields,
            &function_symbols,
            &aliases,
            None,
            &[],
            None,
        )
        .expect("compile runtime bridge");
        let mut objects = bundle.object_paths().cloned().collect::<Vec<_>>();
        objects.push(bridge);
        let linked = temp_root.join("bounded_performance.dll");
        let dynload = ensure_stasis_dynload_link_library().expect("stasis dynload link library");
        stasis_jit::link_objects_to_dynamic_library(
            &objects,
            &linked,
            &[
                "main".to_string(),
                "tick".to_string(),
                "stasis_aot_bind_runtime_globals".to_string(),
            ],
            &AotLinkConfig {
                linker_path: Some(linker_path),
                runtime_lib_paths: vec![dynload.clone()],
                target: stasis_jit::AotTarget::default(),
            },
        )
        .expect("link bounded-performance AOT bundle");
        stage_stasis_dynload_runtime(&dynload, &linked).expect("stage runtime");
        sign_output_artifact_if_configured(&linked).expect("sign bounded-performance AOT bundle");

        let library = DynamicLibrary::load(&linked).expect("load AOT bundle");
        let bind = library
            .symbol_address("stasis_aot_bind_runtime_globals")
            .expect("resolve runtime binding");
        stasis_dynload::invoke_noarg_void(bind).expect("bind runtime globals");
        let main = library.symbol_address("main").expect("resolve main");
        let tick = library.symbol_address("tick").expect("resolve tick");
        assert_eq!(stasis_dynload::invoke_noarg_i32(main).expect("run main"), 0);
        assert_eq!(stasis_dynload::invoke_noarg_i32(tick).expect("run tick"), 0);
        std::mem::forget(library);
        fs::remove_dir_all(&temp_root).ok();
    }
    fn write_fake_linker(temp_root: &Path) -> PathBuf {
        if cfg!(windows) {
            let linker = temp_root.join("fake-link.cmd");
            let script = r#"@echo off
setlocal EnableDelayedExpansion
set OUT=
for %%A in (%*) do (
  set ARG=%%~A
  if "!ARG:~0,1!"=="@" (
    for /f "usebackq delims=" %%R in ("!ARG:~1!") do (
      echo %%R | findstr /B /C:"/OUT:" >nul
      if !errorlevel! == 0 (
        set OUT=%%R
        set OUT=!OUT:~5!
      )
    )
  ) else (
    echo !ARG! | findstr /B /C:"/OUT:" >nul
    if !errorlevel! == 0 (
      set OUT=!ARG:~5!
    )
  )
)
if "%OUT%"=="" exit /b 2
echo fake-dll>"%OUT%"
exit /b 0
"#;
            fs::write(&linker, script).expect("write fake linker script");
            linker
        } else {
            let linker = temp_root.join("fake-link.sh");
            let script = r#"#!/usr/bin/env sh
OUT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      OUT="$2"
      shift
      ;;
  esac
  shift
done
if [ -z "$OUT" ]; then
  exit 2
fi
echo "fake-shared" > "$OUT"
"#;
            fs::write(&linker, script).expect("write fake linker script");
            let status = Command::new("chmod")
                .arg("+x")
                .arg(&linker)
                .status()
                .expect("chmod fake linker");
            assert!(status.success(), "chmod fake linker should succeed");
            linker
        }
    }

    fn write_fake_signer(temp_root: &Path) -> PathBuf {
        if cfg!(windows) {
            let signer = temp_root.join("fake-sign.cmd");
            let script = r#"@echo off
if "%~1"=="" exit /b 2
echo signed>"%~1.signed"
exit /b 0
"#;
            fs::write(&signer, script).expect("write fake signer script");
            signer
        } else {
            let signer = temp_root.join("fake-sign.sh");
            let script = r#"#!/usr/bin/env sh
if [ -z "$1" ]; then
  exit 2
fi
echo "signed" > "$1.signed"
"#;
            fs::write(&signer, script).expect("write fake signer script");
            let status = Command::new("chmod")
                .arg("+x")
                .arg(&signer)
                .status()
                .expect("chmod fake signer");
            assert!(status.success(), "chmod fake signer should succeed");
            signer
        }
    }

    fn new_self_host_test_backend(
        artifact_root: PathBuf,
        linker_path: PathBuf,
    ) -> IncrementalCompilerBackend {
        IncrementalCompilerBackend::with_aot_compile_and_link_config(
            AotCompileConfig::default(),
            AotLinkConfig {
                linker_path: Some(linker_path),
                runtime_lib_paths: vec![],
                target: stasis_jit::AotTarget::default(),
            },
            artifact_root,
            false,
        )
    }

    #[test]
    fn self_host_aot_cli_links_runnable_executable_with_main_entry_symbol() {
        let _global_guard = crate::jit_test_support::lock();
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _signing_environment = disable_ambient_signing();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_aot_cli_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");

        let linker = write_fake_linker(&temp_root);
        let mut backend = new_self_host_test_backend(temp_root.join("aot_artifacts"), linker);
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let summary = run_self_host_aot_cli_with_backend_and_options(
            &mut backend,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::default(),
        )
        .expect("self-host aot cli should succeed");
        assert_eq!(summary.source_file_count, 1);
        assert!(!summary.entry_symbol.is_empty());
        assert_eq!(summary.linked_image_path, output_exe);
        assert!(summary.linked_image_path.exists());
        assert!(summary.ir_bundle_path.as_os_str().is_empty());
        assert!(summary.object_bundle_path.exists());
        assert!(!summary.object_file_names.is_empty());
        assert!(summary
            .object_file_names
            .iter()
            .all(|name| !name.is_empty()));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_links_standalone_storage_for_non_engine_globals() {
        let _global_guard = crate::jit_test_support::lock();
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _signing_environment = disable_ambient_signing();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_storage_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        fs::write(
            project_dir.join("main.stasis"),
            "global count: i32;\nfunction main(): i32 { count = 7; return count; }\n",
        )
        .expect("write source");

        let linker = write_fake_linker(&temp_root);
        let mut backend = new_self_host_test_backend(temp_root.join("aot_artifacts"), linker);
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let summary = run_self_host_aot_cli_with_backend_and_options(
            &mut backend,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::default(),
        )
        .expect("non-engine globals should link through standalone storage");
        assert_eq!(summary.entry_symbol, "stasis_aot_standalone_entry");
        assert!(summary
            .object_file_names
            .iter()
            .any(|name| name == "direct_storage.obj"));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_invokes_signer_when_configured() {
        let _global_guard = crate::jit_test_support::lock();
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = SIGN_ENV_LOCK.lock().expect("lock signer env");
        let _signing_environment = disable_ambient_signing();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_sign_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");

        let linker = write_fake_linker(&temp_root);
        let signer = write_fake_signer(&temp_root);
        let old_signer = std::env::var("STASIS_AOT_SIGN_TOOL").ok();
        std::env::set_var("STASIS_AOT_SIGN_TOOL", &signer);

        let mut backend = new_self_host_test_backend(temp_root.join("aot_artifacts"), linker);
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let result = run_self_host_aot_cli_with_backend_and_options(
            &mut backend,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::default(),
        );
        if let Some(value) = old_signer {
            std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
        } else {
            std::env::remove_var("STASIS_AOT_SIGN_TOOL");
        }

        if cfg!(windows) {
            assert!(result
                .expect_err("a marker-only hook must not pass Authenticode verification")
                .contains("Authenticode verification failed"));
        } else {
            result.expect("self-host signing run should succeed");
        }
        let signed_marker = output_exe.with_file_name(format!(
            "{}.signed",
            output_exe
                .file_name()
                .expect("output file name")
                .to_string_lossy()
        ));
        assert!(signed_marker.exists());

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn missing_optional_signer_does_not_block_unsigned_local_artifacts() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = SIGN_ENV_LOCK.lock().expect("lock signer env");
        let _signing_environment = disable_ambient_signing();
        let old_signer = std::env::var_os("STASIS_AOT_SIGN_TOOL");
        let old_required = std::env::var_os("STASIS_REQUIRE_SIGNED_EXECUTION");
        let missing_signer = std::env::temp_dir().join("stasis-missing-sign-tool");
        std::env::set_var("STASIS_AOT_SIGN_TOOL", &missing_signer);
        std::env::remove_var("STASIS_REQUIRE_SIGNED_EXECUTION");

        let result = sign_output_artifact_if_configured(Path::new("local-artifact.exe"));

        if let Some(value) = old_signer {
            std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
        } else {
            std::env::remove_var("STASIS_AOT_SIGN_TOOL");
        }
        if let Some(value) = old_required {
            std::env::set_var("STASIS_REQUIRE_SIGNED_EXECUTION", value);
        } else {
            std::env::remove_var("STASIS_REQUIRE_SIGNED_EXECUTION");
        }
        result.expect("an unavailable optional signer should permit unsigned local output");
    }

    #[test]
    fn missing_required_signer_fails_before_artifact_execution() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = SIGN_ENV_LOCK.lock().expect("lock signer env");
        let old_signer = std::env::var_os("STASIS_AOT_SIGN_TOOL");
        let old_required = std::env::var_os("STASIS_REQUIRE_SIGNED_EXECUTION");
        let missing_signer = std::env::temp_dir().join("stasis-missing-required-sign-tool");
        std::env::set_var("STASIS_AOT_SIGN_TOOL", &missing_signer);
        std::env::set_var("STASIS_REQUIRE_SIGNED_EXECUTION", "1");

        let result = sign_output_artifact_if_configured(Path::new("local-artifact.exe"));

        if let Some(value) = old_signer {
            std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
        } else {
            std::env::remove_var("STASIS_AOT_SIGN_TOOL");
        }
        if let Some(value) = old_required {
            std::env::set_var("STASIS_REQUIRE_SIGNED_EXECUTION", value);
        } else {
            std::env::remove_var("STASIS_REQUIRE_SIGNED_EXECUTION");
        }
        assert!(result
            .expect_err("required signing must reject an unavailable signer")
            .contains("does not exist"));
    }

    #[test]
    fn self_host_aot_cli_writes_default_summary_sidecar() {
        let _global_guard = crate::jit_test_support::lock();
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _signing_environment = disable_ambient_signing();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_summary_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");

        let linker = write_fake_linker(&temp_root);
        let mut backend = new_self_host_test_backend(temp_root.join("aot_artifacts"), linker);
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let summary = run_self_host_aot_cli_with_backend_and_options(
            &mut backend,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::default(),
        )
        .expect("self-host summary sidecar run should succeed");
        let sidecar_path = default_aot_cli_summary_sidecar_path(&output_exe);
        assert!(sidecar_path.exists());
        let sidecar_text = fs::read_to_string(&sidecar_path).expect("read sidecar");
        let sidecar_summary: SelfHostedAotCliSummary =
            serde_json::from_str(&sidecar_text).expect("parse sidecar summary");
        assert_eq!(sidecar_summary.source_file_count, summary.source_file_count);
        assert_eq!(sidecar_summary.entry_symbol, summary.entry_symbol);
        assert_eq!(sidecar_summary.object_file_names, summary.object_file_names);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_writes_summary_to_configured_path() {
        let _global_guard = crate::jit_test_support::lock();
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _signing_environment = disable_ambient_signing();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_summary_cfg_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");

        let configured_summary = temp_root.join("custom").join("summary.json");
        let linker = write_fake_linker(&temp_root);
        let mut backend = new_self_host_test_backend(temp_root.join("aot_artifacts"), linker);
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let summary = run_self_host_aot_cli_with_backend_and_options(
            &mut backend,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::new(Some(configured_summary.clone()), None),
        )
        .expect("self-host summary configured-path run should succeed");
        assert!(configured_summary.exists());
        let sidecar_text =
            fs::read_to_string(&configured_summary).expect("read configured summary");
        let sidecar_summary: SelfHostedAotCliSummary =
            serde_json::from_str(&sidecar_text).expect("parse configured summary");
        assert_eq!(sidecar_summary.source_file_count, summary.source_file_count);
        assert_eq!(sidecar_summary.entry_symbol, summary.entry_symbol);
        assert_eq!(sidecar_summary.object_file_names, summary.object_file_names);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_is_deterministic_across_repeated_runs_with_same_source() {
        let _global_guard = crate::jit_test_support::lock();
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _signing_environment = disable_ambient_signing();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_aot_determinism_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper() + 5; }\n",
        )
        .expect("write source");

        let linker = write_fake_linker(&temp_root);
        let artifact_root = temp_root.join("aot_artifacts");
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let mut backend_first = new_self_host_test_backend(artifact_root.clone(), linker.clone());
        let first = run_self_host_aot_cli_with_backend_and_options(
            &mut backend_first,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::default(),
        )
        .expect("first run should succeed");

        let mut backend_second = new_self_host_test_backend(artifact_root.clone(), linker);
        let second = run_self_host_aot_cli_with_backend_and_options(
            &mut backend_second,
            &project_dir,
            &output_exe,
            &SelfHostedAotCliOptions::default(),
        )
        .expect("second run should succeed");

        assert_eq!(first.source_file_count, second.source_file_count);
        assert_eq!(first.entry_symbol, second.entry_symbol);
        assert_eq!(first.linked_image_path, second.linked_image_path);
        assert_eq!(first.object_file_names, second.object_file_names);
        assert!(first.ir_bundle_path.as_os_str().is_empty());
        assert!(first.object_bundle_path.exists());

        fs::remove_dir_all(&temp_root).ok();
    }
}

fn collect_stasis_files_recursive(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to read directory entry in {}: {error}",
                    dir.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                format!("failed to read file type for {}: {error}", path.display())
            })?;
            if file_type.is_dir() {
                walk(&path, out)?;
            } else if file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
            {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_stasis_files_for_self_host_project_with_entry(
    root: &Path,
    entry_file: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let Some(entry_file) = entry_file else {
        return collect_stasis_files_recursive(root);
    };
    let root_canonical = root.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize project root {}: {error}",
            root.display()
        )
    })?;
    let requested_entry = PathBuf::from(entry_file);
    let entry_path = if requested_entry.is_absolute() {
        requested_entry
    } else {
        root.join(requested_entry)
    };
    if !entry_path.exists() {
        return Err(format!(
            "entry file does not exist: {}",
            entry_path.display()
        ));
    }
    let entry_canonical = entry_path.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize entry file {}: {error}",
            entry_path.display()
        )
    })?;
    if !entry_canonical.starts_with(&root_canonical) {
        return Err(format!(
            "entry file {} must be within project dir {}",
            entry_canonical.display(),
            root_canonical.display()
        ));
    }

    let (graph, _) = stasis_compiler::frontend::module_graph::load_project_module_graph(
        &root_canonical,
        &entry_canonical,
    )
    .map_err(|diagnostic| diagnostic.message)?;
    let mut files: Vec<PathBuf> = graph
        .modules()
        .keys()
        .map(|path| root_canonical.join(path))
        .collect();
    files.sort();
    Ok(files)
}

fn write_object_bundle_manifest(
    output_dir: &Path,
    entry_symbol: &str,
    object_paths: &[PathBuf],
) -> Result<PathBuf, String> {
    let bundle = SelfHostObjectBundle {
        entry_symbol: entry_symbol.to_string(),
        object_paths: object_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    };
    let bundle_path = output_dir.join("object_bundle_manifest.json");
    let json = serde_json::to_string_pretty(&bundle)
        .map_err(|error| format!("failed to serialize object bundle metadata: {error}"))?;
    std::fs::write(&bundle_path, json).map_err(|error| {
        format!(
            "failed to write object bundle metadata {}: {error}",
            bundle_path.display()
        )
    })?;
    Ok(bundle_path)
}

fn run_self_host_aot_cli_with_backend_and_options(
    backend: &mut IncrementalCompilerBackend,
    project_dir: &Path,
    output_exe: &Path,
    options: &SelfHostedAotCliOptions,
) -> Result<SelfHostedAotCliSummary, String> {
    if !project_dir.exists() {
        return Err(format!(
            "project directory does not exist: {}",
            project_dir.display()
        ));
    }
    let project_root = stable_absolute_path(project_dir);
    if let Some(existing) = backend.project_root.as_ref() {
        if existing != &project_root {
            return Err(format!(
                "compiler project root is immutable (existing {}, requested {})",
                existing.display(),
                project_root.display()
            ));
        }
    } else {
        backend.project_root = Some(project_root);
    }
    let changed_files = collect_stasis_files_for_self_host_project_with_entry(
        project_dir,
        options.entry_file.as_deref(),
    )?;
    if changed_files.is_empty() {
        return Err(format!(
            "no .stasis files found under {}",
            project_dir.display()
        ));
    }

    backend.last_jit_engine_package = None;
    backend.last_aot_engine_bundle = None;
    backend.refresh_cached_sources(&changed_files)?;
    let mut candidate =
        backend.compile_aot_process_from_source_cache(ReachabilityPolicy::Release)?;
    let function_entries = snapshot_function_entries(
        candidate
            .program_snapshot()
            .expect("compiled self-host AOT candidate snapshot"),
    );
    let use_engine_mode_contracts = function_entries.iter().any(is_zero_argument_tick)
        && function_entries.iter().any(|entry| entry.name == "render");

    let mut summary = if use_engine_mode_contracts {
        let bundle_output_dir = backend
            .aot_artifact_root
            .join("engine_bundle")
            .join("request_1");
        if bundle_output_dir.exists() {
            std::fs::remove_dir_all(&bundle_output_dir).map_err(|error| {
                format!(
                    "failed to clear existing AOT engine bundle directory {}: {error}",
                    bundle_output_dir.display()
                )
            })?;
        }
        let bundle = candidate.write_engine_bundle(
            &IncrementalCompilerBackend::engine_entrypoints(false),
            &bundle_output_dir,
        )?;
        backend.last_program_snapshot = candidate.program_snapshot().cloned();
        backend.last_aot_engine_bundle = Some(bundle.clone());
        package_engine_bundle_release(
            backend,
            &bundle,
            output_exe,
            project_dir,
            options.entry_file.as_deref(),
            options.desktop_network.as_ref(),
        )?
    } else {
        let main_entries: Vec<_> = function_entries
            .iter()
            .filter(|entry| entry.name == "main")
            .collect();
        if main_entries.len() != 1 {
            return Err(format!(
                "host ABI alias 'main' requires exactly one canonical identity (found {})",
                main_entries.len()
            ));
        }
        let main_artifact = candidate
            .artifacts()
            .iter()
            .find(|artifact| artifact.function_id == main_entries[0].fn_id.0)
            .ok_or_else(|| "missing compiled artifact for function main(): i32".to_string())?;
        let standalone_storage =
            candidate.compile_standalone_storage_object(&main_artifact.symbol_name)?;
        let compile = backend.compile_aot_non_engine_artifacts_from_process(candidate, 1)?;
        let Some((entry_symbol, _)) = compile
            .object_paths_by_function
            .get(&main_entries[0].fn_id.0)
        else {
            return Err("missing function main(): i32".to_string());
        };
        let mut entry_symbol = entry_symbol.clone();
        let mut object_paths: Vec<PathBuf> = compile
            .object_paths_by_function
            .values()
            .map(|(_, path)| path.clone())
            .collect();
        if let Some((storage_bytes, wrapper_symbol)) = standalone_storage {
            let storage_path = compile.output_dir.join("direct_storage.obj");
            std::fs::write(&storage_path, storage_bytes).map_err(|error| {
                format!(
                    "failed to write standalone AOT storage object {}: {error}",
                    storage_path.display()
                )
            })?;
            object_paths.push(storage_path);
            entry_symbol = wrapper_symbol;
        }
        let mut link_config = backend.aot_link_config.clone();
        let mut dynload_link_library = None;
        if should_link_stasis_dynload(&link_config.target) {
            let link_library = ensure_stasis_dynload_link_library()?;
            link_config.runtime_lib_paths.push(link_library.clone());
            dynload_link_library = Some(link_library);
        }
        link_objects_to_executable(&object_paths, output_exe, &entry_symbol, &link_config)?;
        if let Some(link_library) = dynload_link_library.as_deref() {
            stage_stasis_dynload_runtime(link_library, output_exe)?;
        }
        maybe_sign_output_executable(output_exe)?;
        let object_bundle_path =
            write_object_bundle_manifest(&compile.output_dir, &entry_symbol, &object_paths)?;
        let object_file_names = object_paths
            .iter()
            .map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect();
        SelfHostedAotCliSummary {
            source_file_count: changed_files.len(),
            linked_image_path: output_exe.to_path_buf(),
            entry_symbol,
            ir_bundle_path: PathBuf::new(),
            object_bundle_path,
            object_file_names,
            program_snapshot: None,
        }
    };

    summary.source_file_count = changed_files.len();
    let is_packaged_output = packaged_launch_sidecar_path(&summary.linked_image_path)
        .ok()
        .is_some_and(|path| path.exists());
    if options.summary_file_path.is_some() || !is_packaged_output {
        write_default_aot_cli_summary_sidecar(&summary, options.summary_file_path.as_deref())?;
    }
    Ok(summary)
}

pub fn run_self_host_aot_cli_with_options(
    project_dir: &Path,
    output_exe: &Path,
    summary_file_path: Option<&Path>,
    entry_file: Option<&Path>,
) -> Result<SelfHostedAotCliSummary, String> {
    run_self_host_aot_cli_with_cli_options(
        project_dir,
        output_exe,
        SelfHostedAotCliOptions::new(
            summary_file_path.map(PathBuf::from),
            entry_file.map(PathBuf::from),
        ),
    )
}

fn default_self_host_aot_artifact_root(project_dir: &Path, output_exe: &Path) -> PathBuf {
    let output_key = output_exe
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("aot_output");
    project_dir
        .join(".stasis_cache")
        .join("aot_cli")
        .join(output_key)
}

fn resolve_self_host_aot_artifact_root(
    project_dir: &Path,
    output_exe: &Path,
    override_root: Option<&Path>,
) -> PathBuf {
    override_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_self_host_aot_artifact_root(project_dir, output_exe))
}

fn run_self_host_aot_cli_with_cli_options(
    project_dir: &Path,
    output_exe: &Path,
    options: SelfHostedAotCliOptions,
) -> Result<SelfHostedAotCliSummary, String> {
    let artifact_root = resolve_self_host_aot_artifact_root(
        project_dir,
        output_exe,
        options.artifact_root.as_deref(),
    );
    let mut backend = IncrementalCompilerBackend::new_self_host_aot_cli(artifact_root);
    let mut summary = run_self_host_aot_cli_with_backend_and_options(
        &mut backend,
        project_dir,
        output_exe,
        &options,
    )?;
    summary.program_snapshot = backend.last_program_snapshot.clone();
    Ok(summary)
}

pub fn run_self_host_aot_cli_with_options_and_artifact_root(
    project_dir: &Path,
    output_exe: &Path,
    summary_file_path: Option<&Path>,
    entry_file: Option<&Path>,
    artifact_root: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    run_self_host_aot_cli_with_cli_options(
        project_dir,
        output_exe,
        SelfHostedAotCliOptions::new(
            summary_file_path.map(PathBuf::from),
            entry_file.map(PathBuf::from),
        )
        .with_artifact_root(artifact_root.to_path_buf()),
    )
}

pub fn run_self_host_aot_cli_with_desktop_network(
    project_dir: &Path,
    output_exe: &Path,
    entry_file: &Path,
    library: &Path,
    include_dir: &Path,
    mode: DesktopNetworkMode,
) -> Result<SelfHostedAotCliSummary, String> {
    let options = SelfHostedAotCliOptions::new(None, Some(entry_file.to_path_buf()))
        .with_desktop_network(library.to_path_buf(), include_dir.to_path_buf(), mode);
    run_self_host_aot_cli_with_cli_options(project_dir, output_exe, options)
}

pub fn run_self_host_aot_cli_with_desktop_network_and_artifact_root(
    project_dir: &Path,
    output_exe: &Path,
    entry_file: &Path,
    library: &Path,
    include_dir: &Path,
    mode: DesktopNetworkMode,
    artifact_root: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    let options = SelfHostedAotCliOptions::new(None, Some(entry_file.to_path_buf()))
        .with_desktop_network(library.to_path_buf(), include_dir.to_path_buf(), mode)
        .with_artifact_root(artifact_root.to_path_buf());
    run_self_host_aot_cli_with_cli_options(project_dir, output_exe, options)
}

pub fn run_self_host_aot_cli(
    project_dir: &Path,
    output_exe: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    run_self_host_aot_cli_with_options(project_dir, output_exe, None, None)
}

pub fn sign_output_artifact_if_configured(artifact_path: &Path) -> Result<(), String> {
    crate::windows_signing::sign_output_artifact_if_configured(artifact_path)
}

fn maybe_sign_output_executable(output_exe: &Path) -> Result<(), String> {
    sign_output_artifact_if_configured(output_exe)
}

fn resolve_aot_cli_summary_sidecar_path(
    output_exe: &Path,
    configured_summary_path: Option<&Path>,
) -> PathBuf {
    if let Some(path) = configured_summary_path {
        return path.to_path_buf();
    }
    let file_name = output_exe
        .file_name()
        .map(|name| format!("{}.summary.json", name.to_string_lossy()))
        .unwrap_or_else(|| "aot_cli.summary.json".to_string());
    output_exe.with_file_name(file_name)
}

#[cfg(test)]
fn default_aot_cli_summary_sidecar_path(output_exe: &Path) -> PathBuf {
    let configured_summary_path = std::env::var_os("STASIS_AOT_SUMMARY_FILE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    resolve_aot_cli_summary_sidecar_path(output_exe, configured_summary_path.as_deref())
}

fn write_default_aot_cli_summary_sidecar(
    summary: &SelfHostedAotCliSummary,
    configured_summary_path: Option<&Path>,
) -> Result<(), String> {
    let sidecar_path =
        resolve_aot_cli_summary_sidecar_path(&summary.linked_image_path, configured_summary_path);
    if let Some(parent) = sidecar_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create aot-cli sidecar summary directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(summary)
        .map_err(|error| format!("failed to serialize aot-cli sidecar summary: {error}"))?;
    std::fs::write(&sidecar_path, json).map_err(|error| {
        format!(
            "failed to write aot-cli sidecar summary {}: {error}",
            sidecar_path.display()
        )
    })
}

#[cfg(test)]
mod self_host_file_selection_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTestRoot(PathBuf);

    impl TempTestRoot {
        fn new(label: &str) -> Self {
            let id = TEMP_ROOT_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "stasis_self_host_{label}_{}_{}",
                std::process::id(),
                id
            ));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTestRoot {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn write_self_host_inputs(root: &Path) {
        fs::create_dir_all(root.join("runtime")).expect("create runtime directory");
        fs::create_dir_all(root.join("mobile/shells/common"))
            .expect("create mobile shell directory");
        fs::write(root.join(SELF_HOST_RUNTIME_CMAKE), "installed runtime\n")
            .expect("write runtime input");
        fs::write(root.join(SELF_HOST_MOBILE_MAIN), "installed mobile shell\n")
            .expect("write mobile input");
    }

    #[test]
    fn self_host_repo_root_prefers_installed_executable_root() {
        let root = TempTestRoot::new("root_prefer");
        let compile_root = root.path().join("compile");
        let installed_root = root.path().join("installed");
        write_self_host_inputs(&compile_root);
        write_self_host_inputs(&installed_root);

        let executable = installed_root.join("stasis.exe");
        let resolved = resolve_self_host_repo_root(Some(&executable), &compile_root)
            .expect("installed root should resolve");
        assert_eq!(resolved, installed_root);
    }

    #[test]
    fn self_host_repo_root_resolves_published_bin_bundle_root() {
        let root = TempTestRoot::new("bin_bundle");
        let compile_root = root.path().join("compile");
        let bundle_root = root.path().join("installed");
        write_self_host_inputs(&compile_root);
        write_self_host_inputs(&bundle_root);
        fs::create_dir_all(bundle_root.join("bin")).expect("create bin directory");

        let executable = bundle_root.join("bin/stasis");
        let resolved = resolve_self_host_repo_root(Some(&executable), &compile_root)
            .expect("bundle root should resolve");
        assert_eq!(resolved, bundle_root);
    }

    #[test]
    fn self_host_repo_root_falls_back_when_installed_root_is_incomplete() {
        let root = TempTestRoot::new("direct_fallback");
        let compile_root = root.path().join("compile");
        let installed_root = root.path().join("installed");
        write_self_host_inputs(&compile_root);
        fs::create_dir_all(installed_root.join("runtime")).expect("create partial runtime");
        fs::write(
            installed_root.join(SELF_HOST_RUNTIME_CMAKE),
            "partial runtime\n",
        )
        .expect("write partial input");

        let executable = installed_root.join("stasis.exe");
        let resolved = resolve_self_host_repo_root(Some(&executable), &compile_root)
            .expect("compile root should resolve");
        assert_eq!(resolved, compile_root);
    }

    #[test]
    fn self_host_repo_root_falls_back_when_installed_bin_layout_is_incomplete() {
        let root = TempTestRoot::new("bin_fallback");
        let compile_root = root.path().join("compile");
        let bundle_root = root.path().join("installed");
        let executable_directory = bundle_root.join("bin");
        write_self_host_inputs(&compile_root);
        fs::create_dir_all(executable_directory.join("runtime"))
            .expect("create partial bin runtime");
        fs::write(
            executable_directory.join(SELF_HOST_RUNTIME_CMAKE),
            "partial bin runtime\n",
        )
        .expect("write partial bin input");
        fs::create_dir_all(bundle_root.join("mobile/shells/common"))
            .expect("create partial bundle mobile shell");
        fs::write(
            bundle_root.join(SELF_HOST_MOBILE_MAIN),
            "partial bundle mobile shell\n",
        )
        .expect("write partial bundle input");

        let executable = executable_directory.join("stasis");
        let resolved = resolve_self_host_repo_root(Some(&executable), &compile_root)
            .expect("compile root should resolve");
        assert_eq!(resolved, compile_root);
    }

    #[test]
    fn self_host_repo_root_falls_back_when_executable_is_unavailable() {
        let root = TempTestRoot::new("unavailable_fallback");
        let compile_root = root.path().join("compile");
        write_self_host_inputs(&compile_root);

        let resolved = resolve_self_host_repo_root(None, &compile_root)
            .expect("compile root should resolve without an executable");
        assert_eq!(resolved, compile_root);
    }

    #[test]
    fn self_host_repo_root_reports_all_checked_roots_and_markers() {
        let root = TempTestRoot::new("missing");
        let compile_root = root.path().join("compile");
        let bundle_root = root.path().join("installed");
        let executable_directory = bundle_root.join("bin");
        let executable = executable_directory.join("stasis");

        let error = resolve_self_host_repo_root(Some(&executable), &compile_root)
            .expect_err("missing roots should report an actionable error");
        assert!(error.contains("checked root locations"));
        assert!(error.contains(&executable_directory.display().to_string()));
        assert!(error.contains(&bundle_root.display().to_string()));
        assert!(error.contains(&compile_root.display().to_string()));
        assert!(error.contains("runtime/CMakeLists.txt"));
        assert!(error.contains("mobile/shells/common/stasis_mobile_main.c"));
    }

    #[test]
    fn self_host_project_entry_selects_project_local_import_closure() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_entry_select_{stamp}"));
        fs::create_dir_all(&root).expect("create root");
        fs::write(
            root.join("entry.stasis"),
            "import \"./dep.stasis\";\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write entry");
        fs::write(
            root.join("dep.stasis"),
            "function helper(): i32 { return 1; }\n",
        )
        .expect("write dep");
        fs::write(
            root.join("other.stasis"),
            "function main(): i32 { return 9; }\n",
        )
        .expect("write other");

        let files = collect_stasis_files_for_self_host_project_with_entry(
            &root,
            Some(Path::new("entry.stasis")),
        )
        .expect("collect should succeed");
        let names: Vec<String> = files
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .collect();
        assert!(names.contains(&"entry.stasis".to_string()));
        assert!(names.contains(&"dep.stasis".to_string()));
        assert!(!names.contains(&"other.stasis".to_string()));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn self_host_project_entry_rejects_missing_entry_file() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_entry_missing_{stamp}"));
        fs::create_dir_all(&root).expect("create root");
        let error = collect_stasis_files_for_self_host_project_with_entry(
            &root,
            Some(Path::new("missing_entry.stasis")),
        )
        .expect_err("missing entry should fail");
        assert!(error.contains("entry file does not exist"));
        fs::remove_dir_all(&root).ok();
    }
}
