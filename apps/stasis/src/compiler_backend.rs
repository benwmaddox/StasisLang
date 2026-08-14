use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stasis_compiler::backend::aot::{AotEngineBundle, AotProcess};
use stasis_compiler::backend::jit::{JitEnginePackage, JitProcess};
use stasis_compiler::backend::program_snapshot::ProgramSnapshot;
use stasis_compiler::backend::state_layout::StateLayout;
use stasis_compiler::backend::{AotOptimizationProfile, EngineEntrypoints};
#[cfg(test)]
use stasis_compiler::{SimpleI32Condition, SimpleI32ReturnExpr};
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
struct AotFallbackStubDetail {
    symbol: String,
    id_hash: i32,
    sig_hash: i32,
    body_hash: i32,
    ordinal: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AotPatchManifest {
    request_id: u64,
    artifact_paths: Vec<String>,
    linked_image_path: Option<String>,
    linked_image_size_bytes: Option<u64>,
    linked_image_sha256: Option<String>,
    fallback_stub_symbols: Vec<String>,
    fallback_stub_details: Vec<AotFallbackStubDetail>,
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
    symbol_id: String,
    fn_id: FnId,
}

#[derive(Debug, Clone, Deserialize)]
struct EngineBundleManifestFunctionRow {
    function_id: u32,
    symbol_id: String,
    name: String,
    symbol: String,
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
struct EngineBundleManifest {
    #[serde(default)]
    optimization_profile: Option<String>,
    functions: Vec<EngineBundleManifestFunctionRow>,
    #[serde(default)]
    string_literals: Option<Vec<EngineBundleManifestStringLiteralRow>>,
    #[serde(default)]
    collection_max_lengths: Option<Vec<EngineBundleManifestCollectionMaxLengthRow>>,
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

#[derive(Debug, Clone, PartialEq)]
struct PackagedRuntimeField {
    name: String,
    size: usize,
    field_type: String,
    array_count: usize,
    initial_value: Option<serde_json::Value>,
    collection_path: Option<String>,
    collection_field: Option<String>,
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
}

impl SelfHostedAotCliOptions {
    pub fn new(summary_file_path: Option<PathBuf>, entry_file: Option<PathBuf>) -> Self {
        Self {
            summary_file_path,
            entry_file,
        }
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
            let engine = entries.iter().any(|entry| entry.name == "tick")
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
                        message,
                        path: request.changed_files.first().cloned(),
                        line: None,
                        column: None,
                    }],
                ),
            };
        }
        let candidate = match self.compile_aot_process_from_source_cache() {
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
        let has_tick_entrypoint = function_entries.iter().any(|entry| entry.name == "tick");
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
            if matches!(
                entry.name.as_str(),
                "main" | "tick" | "render" | "on_code_swap"
            ) {
                if let Some(previous) = lifecycle_fn_id_by_name.insert(entry.name.clone(), fn_id) {
                    if previous != fn_id {
                        return CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
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
            if matches!(
                entry.name.as_str(),
                "main" | "tick" | "render" | "on_code_swap"
            ) && host_aliases.insert(entry.name.clone(), fn_id).is_some()
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

    fn layout_hash_from_snapshot(&self) -> LayoutHash {
        LayoutHash(
            self.last_program_snapshot
                .as_ref()
                .expect("successful compiler result must have a program snapshot")
                .layout_digest(),
        )
    }

    fn compile_aot_process_from_source_cache(&mut self) -> Result<AotProcess, String> {
        let mut process = AotProcess::with_optimization_profile(
            Self::aot_optimization_profile_from_compile_config(&self.aot_compile_config),
        );
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
            &[],
            &[],
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
        fallback_stub_symbols: &[String],
        fallback_stub_details: &[AotFallbackStubDetail],
    ) -> Result<(), String> {
        let manifest_path = self.aot_artifact_root.join("last_patch_manifest.json");
        let manifest = AotPatchManifest {
            request_id,
            artifact_paths: artifact_paths.to_vec(),
            linked_image_path,
            linked_image_size_bytes,
            linked_image_sha256,
            fallback_stub_symbols: fallback_stub_symbols.to_vec(),
            fallback_stub_details: fallback_stub_details.to_vec(),
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

#[cfg(test)]
fn hash_identifier(name: &str) -> i32 {
    let mut hash: i32 = 216613626;
    for byte in name.bytes() {
        hash = hash
            .wrapping_mul(16777619)
            .wrapping_add(i32::from(byte) + 1);
    }
    hash
}

#[cfg(test)]
fn build_aot_stub_clif(
    function_name: &str,
    return_type: &str,
    simple_expr: Option<&SimpleI32ReturnExpr>,
    fallback_return_value: i32,
    simple_call_target_symbol: Option<&str>,
    simple_i32_return_call_add_delta: Option<i32>,
    simple_call_one_arg_target_symbol: Option<&str>,
    simple_call_one_arg_i32_literal: Option<i32>,
    simple_call_one_arg_arg_call_target_symbol: Option<&str>,
    simple_two_call_left_target_symbol: Option<&str>,
    simple_two_call_right_target_symbol: Option<&str>,
    simple_i32_return_two_call_op_code: Option<i32>,
    simple_void_print_i32_literal: Option<i32>,
    simple_void_print_i32_call_target_symbol: Option<&str>,
    simple_void_print_i32_call_one_arg_arg_call_target_symbol: Option<&str>,
    simple_void_print_i32_call_add_delta: Option<i32>,
) -> String {
    build_aot_stub_clif_for_metric(
        function_name,
        return_type,
        simple_expr,
        fallback_return_value,
        simple_call_target_symbol,
        simple_i32_return_call_add_delta,
        simple_call_one_arg_target_symbol,
        simple_call_one_arg_i32_literal,
        simple_call_one_arg_arg_call_target_symbol,
        None,
        None,
        None,
        None,
        0,
        0,
        false,
        false,
        false,
        false,
        false,
        simple_two_call_left_target_symbol,
        simple_two_call_right_target_symbol,
        simple_i32_return_two_call_op_code,
        simple_void_print_i32_literal,
        simple_void_print_i32_call_target_symbol,
        simple_void_print_i32_call_one_arg_arg_call_target_symbol,
        simple_void_print_i32_call_add_delta,
    )
}

#[cfg(test)]
fn clif_type_for_stasis_param_code(type_code: i32) -> &'static str {
    if type_code == 1 {
        "i32"
    } else {
        "i64"
    }
}

#[cfg(test)]
fn build_aot_stub_clif_for_metric(
    function_name: &str,
    return_type: &str,
    simple_expr: Option<&SimpleI32ReturnExpr>,
    fallback_return_value: i32,
    simple_call_target_symbol: Option<&str>,
    simple_i32_return_call_add_delta: Option<i32>,
    simple_call_one_arg_target_symbol: Option<&str>,
    simple_call_one_arg_i32_literal: Option<i32>,
    simple_call_one_arg_arg_call_target_symbol: Option<&str>,
    simple_call_two_arg_target_symbol: Option<&str>,
    simple_call_three_arg_target_symbol: Option<&str>,
    simple_call_four_arg_target_symbol: Option<&str>,
    simple_call_two_arg_literal_first_second_param_target_symbol: Option<&str>,
    function_param_count: i32,
    function_first_param_type_code: i32,
    simple_call_one_arg_uses_first_param_passthrough: bool,
    simple_call_two_arg_uses_first_second_param_passthrough: bool,
    simple_call_three_arg_uses_first_second_third_param_passthrough: bool,
    simple_call_four_arg_uses_first_second_third_fourth_param_passthrough: bool,
    simple_call_two_arg_uses_literal_first_second_param_passthrough: bool,
    simple_two_call_left_target_symbol: Option<&str>,
    simple_two_call_right_target_symbol: Option<&str>,
    simple_i32_return_two_call_op_code: Option<i32>,
    simple_void_print_i32_literal: Option<i32>,
    simple_void_print_i32_call_target_symbol: Option<&str>,
    simple_void_print_i32_call_one_arg_arg_call_target_symbol: Option<&str>,
    simple_void_print_i32_call_add_delta: Option<i32>,
) -> String {
    if return_type == "void" {
        if let (Some(left_target_symbol), Some(right_target_symbol), Some(op_code)) = (
            simple_two_call_left_target_symbol,
            simple_two_call_right_target_symbol,
            simple_i32_return_two_call_op_code,
        ) {
            let op = if op_code == 2 { "isub" } else { "iadd" };
            return format!(
                "external print_i32(i32) {}\nexternal {left_target_symbol}() -> i32 {}\nexternal {right_target_symbol}() -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = call %{left_target_symbol}()\nv1 = call %{right_target_symbol}()\nv2 = {op} v0, v1\ncall %print_i32(v2)\nreturn\n}}\n",
                aot_call_conv(),
                aot_call_conv(),
                aot_call_conv(),
                aot_call_conv()
            );
        }
        if let Some(call_target_symbol) = simple_void_print_i32_call_target_symbol {
            if let (Some(arg_literal), Some(delta)) = (
                simple_void_print_i32_literal,
                simple_void_print_i32_call_add_delta,
            ) {
                let abs_delta = delta.abs();
                let op = if delta < 0 { "isub" } else { "iadd" };
                return format!(
                    "external print_i32(i32) {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = iconst.i32 {arg_literal}\nv1 = call %{call_target_symbol}(v0)\nv2 = iconst.i32 {abs_delta}\nv3 = {op} v1, v2\ncall %print_i32(v3)\nreturn\n}}\n",
                    aot_call_conv(),
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
            if let Some(arg_call_target_symbol) =
                simple_void_print_i32_call_one_arg_arg_call_target_symbol
            {
                if let Some(delta) = simple_void_print_i32_call_add_delta {
                    let abs_delta = delta.abs();
                    let op = if delta < 0 { "isub" } else { "iadd" };
                    return format!(
                        "external print_i32(i32) {}\nexternal {arg_call_target_symbol}() -> i32 {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = call %{arg_call_target_symbol}()\nv1 = call %{call_target_symbol}(v0)\nv2 = iconst.i32 {abs_delta}\nv3 = {op} v1, v2\ncall %print_i32(v3)\nreturn\n}}\n",
                        aot_call_conv(),
                        aot_call_conv(),
                        aot_call_conv(),
                        aot_call_conv()
                    );
                }
                return format!(
                    "external print_i32(i32) {}\nexternal {arg_call_target_symbol}() -> i32 {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = call %{arg_call_target_symbol}()\nv1 = call %{call_target_symbol}(v0)\ncall %print_i32(v1)\nreturn\n}}\n",
                    aot_call_conv(),
                    aot_call_conv(),
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
            if let Some(arg_literal) = simple_void_print_i32_literal {
                return format!(
                    "external print_i32(i32) {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = iconst.i32 {arg_literal}\nv1 = call %{call_target_symbol}(v0)\ncall %print_i32(v1)\nreturn\n}}\n",
                    aot_call_conv(),
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
            if let Some(delta) = simple_void_print_i32_call_add_delta {
                let abs_delta = delta.abs();
                let op = if delta < 0 { "isub" } else { "iadd" };
                return format!(
                    "external print_i32(i32) {}\nexternal {call_target_symbol}() -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = call %{call_target_symbol}()\nv1 = iconst.i32 {abs_delta}\nv2 = {op} v0, v1\ncall %print_i32(v2)\nreturn\n}}\n",
                    aot_call_conv(),
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
            return format!(
                "external print_i32(i32) {}\nexternal {call_target_symbol}() -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = call %{call_target_symbol}()\ncall %print_i32(v0)\nreturn\n}}\n",
                aot_call_conv(),
                aot_call_conv(),
                aot_call_conv()
            );
        }
        if let Some(print_literal) = simple_void_print_i32_literal {
            return format!(
                "external print_i32(i32) {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = iconst.i32 {print_literal}\ncall %print_i32(v0)\nreturn\n}}\n",
                aot_call_conv(),
                aot_call_conv()
            );
        }
        return format!(
            "function %{function_name}() {} {{\nblock0:\nreturn\n}}\n",
            aot_call_conv()
        );
    }
    if let Some(call_target_symbol) = simple_call_target_symbol {
        if let Some(delta) = simple_i32_return_call_add_delta {
            let abs_delta = delta.abs();
            let op = if delta < 0 { "isub" } else { "iadd" };
            return format!(
                "external {call_target_symbol}() -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = call %{call_target_symbol}()\nv1 = iconst.i32 {abs_delta}\nv2 = {op} v0, v1\nreturn v2\n}}\n",
                aot_call_conv(),
                aot_call_conv()
            );
        }
        return format!(
            "external {call_target_symbol}() -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = call %{call_target_symbol}()\nreturn v0\n}}\n",
            aot_call_conv(),
            aot_call_conv()
        );
    }
    if simple_call_four_arg_uses_first_second_third_fourth_param_passthrough {
        if let Some(call_target_symbol) = simple_call_four_arg_target_symbol {
            if function_param_count == 4 {
                let first_arg_type =
                    clif_type_for_stasis_param_code(function_first_param_type_code);
                let second_arg_type = "i32";
                let third_arg_type = "i64";
                let fourth_arg_type = "i64";
                if let Some(delta) = simple_i32_return_call_add_delta {
                    let abs_delta = delta.abs();
                    let op = if delta < 0 { "isub" } else { "iadd" };
                    return format!(
                        "external {call_target_symbol}({first_arg_type}, {second_arg_type}, {third_arg_type}, {fourth_arg_type}) -> i32 {}\nfunction %{function_name}({first_arg_type}, {second_arg_type}, {third_arg_type}, {fourth_arg_type}) -> i32 {} {{\nblock0:\nv4 = call %{call_target_symbol}(v0, v1, v2, v3)\nv5 = iconst.i32 {abs_delta}\nv6 = {op} v4, v5\nreturn v6\n}}\n",
                        aot_call_conv(),
                        aot_call_conv()
                    );
                }
                return format!(
                    "external {call_target_symbol}({first_arg_type}, {second_arg_type}, {third_arg_type}, {fourth_arg_type}) -> i32 {}\nfunction %{function_name}({first_arg_type}, {second_arg_type}, {third_arg_type}, {fourth_arg_type}) -> i32 {} {{\nblock0:\nv4 = call %{call_target_symbol}(v0, v1, v2, v3)\nreturn v4\n}}\n",
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
        }
    }
    if simple_call_three_arg_uses_first_second_third_param_passthrough {
        if let Some(call_target_symbol) = simple_call_three_arg_target_symbol {
            if function_param_count == 3 {
                let first_arg_type =
                    clif_type_for_stasis_param_code(function_first_param_type_code);
                let second_arg_type = "i64";
                let third_arg_type = "i64";
                if let Some(delta) = simple_i32_return_call_add_delta {
                    let abs_delta = delta.abs();
                    let op = if delta < 0 { "isub" } else { "iadd" };
                    return format!(
                        "external {call_target_symbol}({first_arg_type}, {second_arg_type}, {third_arg_type}) -> i32 {}\nfunction %{function_name}({first_arg_type}, {second_arg_type}, {third_arg_type}) -> i32 {} {{\nblock0:\nv3 = call %{call_target_symbol}(v0, v1, v2)\nv4 = iconst.i32 {abs_delta}\nv5 = {op} v3, v4\nreturn v5\n}}\n",
                        aot_call_conv(),
                        aot_call_conv()
                    );
                }
                return format!(
                    "external {call_target_symbol}({first_arg_type}, {second_arg_type}, {third_arg_type}) -> i32 {}\nfunction %{function_name}({first_arg_type}, {second_arg_type}, {third_arg_type}) -> i32 {} {{\nblock0:\nv3 = call %{call_target_symbol}(v0, v1, v2)\nreturn v3\n}}\n",
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
        }
    }
    if simple_call_two_arg_uses_first_second_param_passthrough {
        if let Some(call_target_symbol) = simple_call_two_arg_target_symbol {
            if function_param_count == 2 {
                let first_arg_type =
                    clif_type_for_stasis_param_code(function_first_param_type_code);
                let second_arg_type = "i64";
                if let Some(delta) = simple_i32_return_call_add_delta {
                    let abs_delta = delta.abs();
                    let op = if delta < 0 { "isub" } else { "iadd" };
                    return format!(
                        "external {call_target_symbol}({first_arg_type}, {second_arg_type}) -> i32 {}\nfunction %{function_name}({first_arg_type}, {second_arg_type}) -> i32 {} {{\nblock0:\nv2 = call %{call_target_symbol}(v0, v1)\nv3 = iconst.i32 {abs_delta}\nv4 = {op} v2, v3\nreturn v4\n}}\n",
                        aot_call_conv(),
                        aot_call_conv()
                    );
                }
                return format!(
                    "external {call_target_symbol}({first_arg_type}, {second_arg_type}) -> i32 {}\nfunction %{function_name}({first_arg_type}, {second_arg_type}) -> i32 {} {{\nblock0:\nv2 = call %{call_target_symbol}(v0, v1)\nreturn v2\n}}\n",
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
        }
    }
    if simple_call_two_arg_uses_literal_first_second_param_passthrough {
        if let (Some(call_target_symbol), Some(arg_literal)) = (
            simple_call_two_arg_literal_first_second_param_target_symbol,
            simple_call_one_arg_i32_literal,
        ) {
            if function_param_count == 1 {
                let second_arg_type =
                    clif_type_for_stasis_param_code(function_first_param_type_code);
                if let Some(delta) = simple_i32_return_call_add_delta {
                    let abs_delta = delta.abs();
                    let op = if delta < 0 { "isub" } else { "iadd" };
                    return format!(
                        "external {call_target_symbol}(i32, {second_arg_type}) -> i32 {}\nfunction %{function_name}({second_arg_type}) -> i32 {} {{\nblock0:\nv1 = iconst.i32 {arg_literal}\nv2 = call %{call_target_symbol}(v1, v0)\nv3 = iconst.i32 {abs_delta}\nv4 = {op} v2, v3\nreturn v4\n}}\n",
                        aot_call_conv(),
                        aot_call_conv()
                    );
                }
                return format!(
                    "external {call_target_symbol}(i32, {second_arg_type}) -> i32 {}\nfunction %{function_name}({second_arg_type}) -> i32 {} {{\nblock0:\nv1 = iconst.i32 {arg_literal}\nv2 = call %{call_target_symbol}(v1, v0)\nreturn v2\n}}\n",
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
        }
    }
    if simple_call_one_arg_uses_first_param_passthrough {
        if let Some(call_target_symbol) = simple_call_one_arg_target_symbol {
            if function_param_count == 1 {
                let arg_type = clif_type_for_stasis_param_code(function_first_param_type_code);
                if let Some(delta) = simple_i32_return_call_add_delta {
                    let abs_delta = delta.abs();
                    let op = if delta < 0 { "isub" } else { "iadd" };
                    return format!(
                        "external {call_target_symbol}({arg_type}) -> i32 {}\nfunction %{function_name}({arg_type}) -> i32 {} {{\nblock0:\nv1 = call %{call_target_symbol}(v0)\nv2 = iconst.i32 {abs_delta}\nv3 = {op} v1, v2\nreturn v3\n}}\n",
                        aot_call_conv(),
                        aot_call_conv()
                    );
                }
                return format!(
                    "external {call_target_symbol}({arg_type}) -> i32 {}\nfunction %{function_name}({arg_type}) -> i32 {} {{\nblock0:\nv1 = call %{call_target_symbol}(v0)\nreturn v1\n}}\n",
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
        }
    }
    if let (Some(call_target_symbol), Some(arg_literal)) = (
        simple_call_one_arg_target_symbol,
        simple_call_one_arg_i32_literal,
    ) {
        if let Some(delta) = simple_i32_return_call_add_delta {
            let abs_delta = delta.abs();
            let op = if delta < 0 { "isub" } else { "iadd" };
            return format!(
                "external {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = iconst.i32 {arg_literal}\nv1 = call %{call_target_symbol}(v0)\nv2 = iconst.i32 {abs_delta}\nv3 = {op} v1, v2\nreturn v3\n}}\n",
                aot_call_conv(),
                aot_call_conv()
            );
        }
        return format!(
            "external {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = iconst.i32 {arg_literal}\nv1 = call %{call_target_symbol}(v0)\nreturn v1\n}}\n",
            aot_call_conv(),
            aot_call_conv()
        );
    }
    if let (Some(call_target_symbol), Some(arg_call_target_symbol)) = (
        simple_call_one_arg_target_symbol,
        simple_call_one_arg_arg_call_target_symbol,
    ) {
        if let Some(delta) = simple_i32_return_call_add_delta {
            let abs_delta = delta.abs();
            let op = if delta < 0 { "isub" } else { "iadd" };
            return format!(
                "external {arg_call_target_symbol}() -> i32 {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = call %{arg_call_target_symbol}()\nv1 = call %{call_target_symbol}(v0)\nv2 = iconst.i32 {abs_delta}\nv3 = {op} v1, v2\nreturn v3\n}}\n",
                aot_call_conv(),
                aot_call_conv(),
                aot_call_conv()
            );
        }
        return format!(
            "external {arg_call_target_symbol}() -> i32 {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = call %{arg_call_target_symbol}()\nv1 = call %{call_target_symbol}(v0)\nreturn v1\n}}\n",
            aot_call_conv(),
            aot_call_conv(),
            aot_call_conv()
        );
    }
    if let (Some(left_target_symbol), Some(right_target_symbol), Some(op_code)) = (
        simple_two_call_left_target_symbol,
        simple_two_call_right_target_symbol,
        simple_i32_return_two_call_op_code,
    ) {
        let op = if op_code == 2 { "isub" } else { "iadd" };
        return format!(
            "external {left_target_symbol}() -> i32 {}\nexternal {right_target_symbol}() -> i32 {}\nfunction %{function_name}() -> i32 {} {{\nblock0:\nv0 = call %{left_target_symbol}()\nv1 = call %{right_target_symbol}()\nv2 = {op} v0, v1\nreturn v2\n}}\n",
            aot_call_conv(),
            aot_call_conv(),
            aot_call_conv()
        );
    }
    let fallback_expr = SimpleI32ReturnExpr::Literal(fallback_return_value);
    let expression = simple_expr.unwrap_or(&fallback_expr);
    let mut temp_counter: u32 = 0;
    let body = if let SimpleI32ReturnExpr::Select(condition, then_expr, else_expr) = expression {
        let mut next_block_id: u32 = 3;
        let mut branch_blocks = emit_clif_condition_branch_blocks(
            condition,
            "block0",
            "block1",
            "block2",
            &mut temp_counter,
            &mut next_block_id,
        );

        let mut then_lines = Vec::new();
        let then_value = emit_clif_for_simple_expr(then_expr, &mut temp_counter, &mut then_lines);
        then_lines.push(format!("return {then_value}"));

        let mut else_lines = Vec::new();
        let else_value = emit_clif_for_simple_expr(else_expr, &mut temp_counter, &mut else_lines);
        else_lines.push(format!("return {else_value}"));

        branch_blocks.push(("block1".to_string(), then_lines));
        branch_blocks.push(("block2".to_string(), else_lines));
        branch_blocks
            .into_iter()
            .map(|(label, lines)| format!("{label}:\n{}", lines.join("\n")))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let mut lines = Vec::new();
        let result = emit_clif_for_simple_expr(expression, &mut temp_counter, &mut lines);
        lines.push(format!("return {result}"));
        format!("block0:\n{}", lines.join("\n"))
    };
    format!(
        "function %{function_name}() -> i32 {} {{\n{body}\n}}\n",
        aot_call_conv()
    )
}

#[cfg(test)]
fn resolve_unique_i32_call_target_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    metrics: &[stasis_compiler::FunctionMetric],
) -> Option<String> {
    let target_id_hash = maybe_target_id_hash?;
    let mut matches = metrics.iter().filter(|candidate| {
        candidate.id_hash == target_id_hash
            && candidate.return_type_code == stasis_compiler::RETURN_TYPE_CODE_I32
            && candidate.param_count == 0
    });
    if let Some(first) = matches.next() {
        if matches.next().is_some() {
            return None;
        }
        return Some(aot_symbol_name(first));
    }
    resolve_known_host_noarg_i32_extern_symbol_by_hash(target_id_hash).map(str::to_string)
}

#[cfg(test)]
fn resolve_unique_i32_single_arg_call_target_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    metrics: &[stasis_compiler::FunctionMetric],
    first_param_type_code: i32,
) -> Option<String> {
    let target_id_hash = maybe_target_id_hash?;
    let mut matches = metrics.iter().filter(|candidate| {
        candidate.id_hash == target_id_hash
            && candidate.return_type_code == stasis_compiler::RETURN_TYPE_CODE_I32
            && candidate.param_count == 1
            && candidate.first_param_type_code == first_param_type_code
    });
    if let Some(first) = matches.next() {
        if matches.next().is_some() {
            return None;
        }
        return Some(aot_symbol_name(first));
    }
    resolve_known_host_single_arg_i32_extern_symbol_by_hash(target_id_hash, first_param_type_code)
        .map(str::to_string)
}

#[cfg(test)]
fn resolve_known_host_noarg_i32_extern_symbol_by_hash(target_id_hash: i32) -> Option<&'static str> {
    if target_id_hash == hash_identifier("host_cli_arg_count") {
        return Some("host_cli_arg_count");
    }
    if target_id_hash == hash_identifier("host_run_self_host_aot_cli_from_env") {
        return Some("host_run_self_host_aot_cli_from_env");
    }
    None
}

#[cfg(test)]
fn resolve_known_host_single_arg_i32_extern_symbol_by_hash(
    target_id_hash: i32,
    first_param_type_code: i32,
) -> Option<&'static str> {
    if first_param_type_code != 0 {
        return None;
    }
    if target_id_hash == hash_identifier("host_source_file_count") {
        return Some("host_source_file_count");
    }
    if target_id_hash == hash_identifier("host_set_summary_file") {
        return Some("host_set_summary_file");
    }
    None
}

#[cfg(test)]
fn resolve_known_host_two_arg_i32_extern_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    first_param_type_code: i32,
) -> Option<&'static str> {
    let target_id_hash = maybe_target_id_hash?;
    if target_id_hash == hash_identifier("host_cli_arg_value") && first_param_type_code == 1 {
        return Some("host_cli_arg_value");
    }
    None
}

#[cfg(test)]
fn resolve_known_host_three_arg_i32_extern_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    first_param_type_code: i32,
) -> Option<&'static str> {
    let target_id_hash = maybe_target_id_hash?;
    if target_id_hash == hash_identifier("host_write_aot_cli_summary") && first_param_type_code == 0
    {
        return Some("host_write_aot_cli_summary");
    }
    None
}

#[cfg(test)]
fn resolve_known_host_four_arg_i32_extern_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    first_param_type_code: i32,
) -> Option<&'static str> {
    let target_id_hash = maybe_target_id_hash?;
    if target_id_hash == hash_identifier("host_load_source_file") && first_param_type_code == 0 {
        return Some("host_load_source_file");
    }
    None
}

#[cfg(test)]
fn resolve_known_host_two_arg_literal_first_second_param_i32_extern_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    first_param_type_code: i32,
) -> Option<&'static str> {
    let target_id_hash = maybe_target_id_hash?;
    if target_id_hash == hash_identifier("host_cli_arg_value") && first_param_type_code == 0 {
        return Some("host_cli_arg_value");
    }
    None
}

#[cfg(test)]
fn emit_clif_for_simple_expr(
    expr: &SimpleI32ReturnExpr,
    temp_counter: &mut u32,
    lines: &mut Vec<String>,
) -> String {
    match expr {
        SimpleI32ReturnExpr::Literal(value) => {
            let name = format!("v{temp_counter}");
            lines.push(format!("{name} = iconst.i32 {value}"));
            *temp_counter += 1;
            name
        }
        SimpleI32ReturnExpr::Add(left, right) => {
            emit_clif_binary_expr("iadd", left, right, temp_counter, lines)
        }
        SimpleI32ReturnExpr::Sub(left, right) => {
            emit_clif_binary_expr("isub", left, right, temp_counter, lines)
        }
        SimpleI32ReturnExpr::Mul(left, right) => {
            emit_clif_binary_expr("imul", left, right, temp_counter, lines)
        }
        SimpleI32ReturnExpr::Div(left, right) => {
            emit_clif_binary_expr("sdiv", left, right, temp_counter, lines)
        }
        SimpleI32ReturnExpr::Mod(left, right) => {
            emit_clif_binary_expr("srem", left, right, temp_counter, lines)
        }
        SimpleI32ReturnExpr::Select(condition, then_expr, else_expr) => {
            let cond_name = emit_clif_condition(condition, temp_counter, lines);
            let then_name = emit_clif_for_simple_expr(then_expr, temp_counter, lines);
            let else_name = emit_clif_for_simple_expr(else_expr, temp_counter, lines);
            let out = format!("v{temp_counter}");
            lines.push(format!(
                "{out} = select {cond_name}, {then_name}, {else_name}"
            ));
            *temp_counter += 1;
            out
        }
    }
}

#[cfg(test)]
fn emit_clif_condition(
    condition: &SimpleI32Condition,
    temp_counter: &mut u32,
    lines: &mut Vec<String>,
) -> String {
    let (opcode, left, right) = match condition {
        SimpleI32Condition::Eq(left, right) => ("eq", left, right),
        SimpleI32Condition::Ne(left, right) => ("ne", left, right),
        SimpleI32Condition::Le(left, right) => ("sle", left, right),
        SimpleI32Condition::Ge(left, right) => ("sge", left, right),
        SimpleI32Condition::Lt(left, right) => ("slt", left, right),
        SimpleI32Condition::Gt(left, right) => ("sgt", left, right),
        SimpleI32Condition::And(left, right) => {
            let left_name = emit_clif_condition(left, temp_counter, lines);
            let right_name = emit_clif_condition(right, temp_counter, lines);
            let out = format!("v{temp_counter}");
            lines.push(format!("{out} = band {left_name}, {right_name}"));
            *temp_counter += 1;
            return out;
        }
        SimpleI32Condition::Or(left, right) => {
            let left_name = emit_clif_condition(left, temp_counter, lines);
            let right_name = emit_clif_condition(right, temp_counter, lines);
            let out = format!("v{temp_counter}");
            lines.push(format!("{out} = bor {left_name}, {right_name}"));
            *temp_counter += 1;
            return out;
        }
        SimpleI32Condition::Not(inner) => {
            let input = emit_clif_condition(inner, temp_counter, lines);
            let out = format!("v{temp_counter}");
            lines.push(format!("{out} = bnot {input}"));
            *temp_counter += 1;
            return out;
        }
    };
    let left_name = emit_clif_for_simple_expr(left, temp_counter, lines);
    let right_name = emit_clif_for_simple_expr(right, temp_counter, lines);
    let out = format!("v{temp_counter}");
    lines.push(format!("{out} = icmp {opcode} {left_name}, {right_name}"));
    *temp_counter += 1;
    out
}

#[cfg(test)]
fn emit_clif_condition_branch_blocks(
    condition: &SimpleI32Condition,
    current_label: &str,
    true_label: &str,
    false_label: &str,
    temp_counter: &mut u32,
    next_block_id: &mut u32,
) -> Vec<(String, Vec<String>)> {
    match condition {
        SimpleI32Condition::Eq(left, right) => vec![emit_clif_comparison_branch_block(
            "eq",
            left,
            right,
            current_label,
            true_label,
            false_label,
            temp_counter,
        )],
        SimpleI32Condition::Ne(left, right) => vec![emit_clif_comparison_branch_block(
            "ne",
            left,
            right,
            current_label,
            true_label,
            false_label,
            temp_counter,
        )],
        SimpleI32Condition::Le(left, right) => vec![emit_clif_comparison_branch_block(
            "sle",
            left,
            right,
            current_label,
            true_label,
            false_label,
            temp_counter,
        )],
        SimpleI32Condition::Ge(left, right) => vec![emit_clif_comparison_branch_block(
            "sge",
            left,
            right,
            current_label,
            true_label,
            false_label,
            temp_counter,
        )],
        SimpleI32Condition::Lt(left, right) => vec![emit_clif_comparison_branch_block(
            "slt",
            left,
            right,
            current_label,
            true_label,
            false_label,
            temp_counter,
        )],
        SimpleI32Condition::Gt(left, right) => vec![emit_clif_comparison_branch_block(
            "sgt",
            left,
            right,
            current_label,
            true_label,
            false_label,
            temp_counter,
        )],
        SimpleI32Condition::And(left, right) => {
            let rhs_label = format!("block{next_block_id}");
            *next_block_id += 1;
            let mut blocks = emit_clif_condition_branch_blocks(
                left,
                current_label,
                &rhs_label,
                false_label,
                temp_counter,
                next_block_id,
            );
            blocks.extend(emit_clif_condition_branch_blocks(
                right,
                &rhs_label,
                true_label,
                false_label,
                temp_counter,
                next_block_id,
            ));
            blocks
        }
        SimpleI32Condition::Or(left, right) => {
            let rhs_label = format!("block{next_block_id}");
            *next_block_id += 1;
            let mut blocks = emit_clif_condition_branch_blocks(
                left,
                current_label,
                true_label,
                &rhs_label,
                temp_counter,
                next_block_id,
            );
            blocks.extend(emit_clif_condition_branch_blocks(
                right,
                &rhs_label,
                true_label,
                false_label,
                temp_counter,
                next_block_id,
            ));
            blocks
        }
        SimpleI32Condition::Not(inner) => emit_clif_condition_branch_blocks(
            inner,
            current_label,
            false_label,
            true_label,
            temp_counter,
            next_block_id,
        ),
    }
}

#[cfg(test)]
fn emit_clif_comparison_branch_block(
    predicate: &str,
    left: &SimpleI32ReturnExpr,
    right: &SimpleI32ReturnExpr,
    current_label: &str,
    true_label: &str,
    false_label: &str,
    temp_counter: &mut u32,
) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    let left_name = emit_clif_for_simple_expr(left, temp_counter, &mut lines);
    let right_name = emit_clif_for_simple_expr(right, temp_counter, &mut lines);
    let cond_name = format!("v{temp_counter}");
    lines.push(format!(
        "{cond_name} = icmp {predicate} {left_name}, {right_name}"
    ));
    *temp_counter += 1;
    lines.push(format!("brif {cond_name}, {true_label}, {false_label}"));
    (current_label.to_string(), lines)
}

#[cfg(test)]
fn emit_clif_binary_expr(
    opcode: &str,
    left: &SimpleI32ReturnExpr,
    right: &SimpleI32ReturnExpr,
    temp_counter: &mut u32,
    lines: &mut Vec<String>,
) -> String {
    let left_name = emit_clif_for_simple_expr(left, temp_counter, lines);
    let right_name = emit_clif_for_simple_expr(right, temp_counter, lines);
    let out = format!("v{temp_counter}");
    lines.push(format!("{out} = {opcode} {left_name}, {right_name}"));
    *temp_counter += 1;
    out
}

#[cfg(test)]
fn aot_symbol_name(metric: &stasis_compiler::FunctionMetric) -> String {
    format!(
        "fn_{}_{}_{}",
        metric.id_hash.unsigned_abs(),
        metric.sig_hash.unsigned_abs(),
        metric.ordinal
    )
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

fn self_host_repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
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
        .unwrap_or_else(|| self_host_repo_root().join("target"));
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
    let repo_root = self_host_repo_root();
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
    let repo_root = self_host_repo_root();
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
            if !source.is_absolute() || !source.exists() || !is_bundleable_asset_extension(&source)
            {
                return Ok(false);
            }

            if let Some(existing) = copied_assets.get(&source) {
                *text = existing.clone();
                return Ok(true);
            }

            let asset_dir = staged_entry_dir.join("assets");
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
            let mut destination = asset_dir.join(file_name);
            if destination.exists() {
                let stem = source
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("asset");
                let ext = source
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("");
                let mut counter: usize = 1;
                while destination.exists() {
                    let candidate = if ext.is_empty() {
                        format!("{stem}_{counter}")
                    } else {
                        format!("{stem}_{counter}.{ext}")
                    };
                    destination = asset_dir.join(candidate);
                    counter += 1;
                }
            }

            copy_file_creating_parent(&source, &destination)?;
            let rel_name = destination
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("invalid packaged asset path {}", destination.display()))?;
            let rel_path = format!("{package_rel_root}/assets/{rel_name}");
            copied_assets.insert(source, rel_path.clone());
            *text = rel_path;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn rewrite_packaged_json_asset_paths(
    staged_entry_dir: &Path,
    package_rel_root: &str,
) -> Result<(), String> {
    fn walk(
        dir: &Path,
        staged_entry_dir: &Path,
        package_rel_root: &str,
        copied_assets: &mut BTreeMap<PathBuf, String>,
    ) -> Result<(), String> {
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
                walk(&path, staged_entry_dir, package_rel_root, copied_assets)?;
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".json") || name.ends_with(".struct-meta.json") {
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
                let next = serde_json::to_string_pretty(&value)
                    .map_err(|error| format!("failed to serialize {}: {error}", path.display()))?;
                std::fs::write(&path, next)
                    .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
            }
        }
        Ok(())
    }

    let mut copied_assets = BTreeMap::new();
    if staged_entry_dir.exists() {
        walk(
            staged_entry_dir,
            staged_entry_dir,
            package_rel_root,
            &mut copied_assets,
        )?;
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
            name: scalar.path.replace('.', "__"),
            size: field_width(storage_type_name)?,
            field_type: storage_type_name.to_string(),
            array_count: 1,
            initial_value,
            collection_path: None,
            collection_field: None,
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
            let name = if field.field.is_empty() {
                collection.path.replace('.', "__")
            } else {
                format!(
                    "{}__{}",
                    collection.path.replace('.', "__"),
                    field.field.replace('.', "__")
                )
            };
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

    let runtime_path = field.name.replace("__", ".");
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

fn build_engine_bundle_runtime_bridge_source(
    target: &stasis_jit::AotTarget,
    runtime_fields: &[PackagedRuntimeField],
    function_symbols: &[String],
    function_aliases: &[PackagedFunctionAlias],
    string_literals: &[EngineBundleManifestStringLiteralRow],
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
STASIS_EXPORT int32_t gfx_cmd_i32[34608] = {0};\n\
STASIS_EXPORT float gfx_cmd_f32[125060] = {0};\n\
STASIS_EXPORT uint8_t gfx_cmd_u8[65536] = {0};\n\
STASIS_EXPORT int32_t host_req_seq = 0;\n\
STASIS_EXPORT int32_t host_req_flags = 0;\n\
STASIS_EXPORT int32_t host_req_window_w_px = 0;\n\
STASIS_EXPORT int32_t host_req_window_h_px = 0;\n",
    );

    let mut register_lines = vec![
        format!(
            "stasis_jit_register_global_i32_array({host_i32_hash}, 0, host_i32, 768);"
        ),
        format!(
            "stasis_jit_register_global_f32_array({host_f32_hash}, 0, host_f32, 64);"
        ),
        format!(
            "stasis_jit_register_global_i32_array({gfx_cmd_i32_hash}, 0, gfx_cmd_i32, 34608);"
        ),
        format!(
            "stasis_jit_register_global_f32_array({gfx_cmd_f32_hash}, 0, gfx_cmd_f32, 125060);"
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
    // Keep the Android ABI surface fixed while the host shell/input event mapping lands separately.
    if target.is_android() {
        source.push_str(
            "STASIS_EXPORT void stasis_init(int width, int height) {\n\
    host_i32[12] = width;\n\
    host_i32[13] = height;\n\
    host_i32[14] = 3;\n\
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
    string_literals: &[EngineBundleManifestStringLiteralRow],
) -> Result<PathBuf, String> {
    let source_path = backend
        .aot_artifact_root
        .join("engine_bundle_runtime_bridge.c");
    let object_path = backend.aot_artifact_root.join(format!(
        "engine_bundle_runtime_bridge.{}",
        runtime_bridge_object_extension(&backend.aot_compile_config.target)
    ));
    let source = build_engine_bundle_runtime_bridge_source(
        &backend.aot_compile_config.target,
        runtime_fields,
        function_symbols,
        function_aliases,
        string_literals,
    )?;
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
        .find(|row| row.name == name)
        .map(|row| row.symbol.clone())
        .ok_or_else(|| format!("engine bundle manifest is missing required symbol {name}"))
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

fn write_macos_runner_info_plist(path: &Path, executable_name: &str) -> Result<(), String> {
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
) -> Result<SelfHostedAotCliSummary, String> {
    let manifest = backend.read_engine_bundle_manifest(&bundle.manifest_path)?;
    let entry_symbol = resolve_engine_bundle_symbol(&manifest, "main")?;
    let tick_symbol = manifest
        .functions
        .iter()
        .find(|row| row.name == "tick")
        .map(|row| row.symbol.clone());
    let render_symbol = manifest
        .functions
        .iter()
        .find(|row| row.name == "render")
        .map(|row| row.symbol.clone());
    let on_code_swap_symbol = manifest
        .functions
        .iter()
        .find(|row| row.name == "on_code_swap")
        .map(|row| row.symbol.clone());

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

    let entry_file = resolve_self_host_aot_entry_file(project_dir, entry_file_override)?;
    let support = stage_entry_support_files(project_dir, entry_file.as_deref(), output_root)?;
    let state_layout = backend
        .last_program_snapshot
        .as_ref()
        .map(ProgramSnapshot::state_layout)
        .ok_or_else(|| "AOT program snapshot missing during packaging".to_string())?;
    let runtime_fields = merge_runtime_fields(state_layout, &support.runtime_fields)?;
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
    if let Some(symbol) = render_symbol.as_ref() {
        function_aliases.push(PackagedFunctionAlias {
            alias: "render",
            target_symbol: symbol.clone(),
            returns_i32: true,
        });
    }
    if let Some(symbol) = on_code_swap_symbol.as_ref() {
        function_aliases.push(PackagedFunctionAlias {
            alias: "on_code_swap",
            target_symbol: symbol.clone(),
            returns_i32: false,
        });
    }

    let mut export_symbols: BTreeSet<String> = BTreeSet::new();
    export_symbols.insert(entry_symbol.clone());
    export_symbols.insert("main".to_string());
    export_symbols.insert("stasis_aot_bind_runtime_globals".to_string());
    if let Some(symbol) = tick_symbol.as_ref() {
        export_symbols.insert(symbol.clone());
        export_symbols.insert("tick".to_string());
    }
    if let Some(symbol) = render_symbol.as_ref() {
        export_symbols.insert(symbol.clone());
        export_symbols.insert("render".to_string());
    }
    if let Some(on_code_swap) = on_code_swap_symbol.as_ref() {
        export_symbols.insert(on_code_swap.clone());
        export_symbols.insert("on_code_swap".to_string());
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
        &string_literals,
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
    if render_symbol.is_some() {
        launch_lines.push("render=render".to_string());
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
        write_macos_runner_info_plist(info_plist, executable_name)?;
    }
    maybe_sign_output_artifact(packaged_output_exe)?;
    maybe_sign_output_artifact(&linked_library_path)?;
    maybe_sign_output_artifact(&graphics_dst)?;
    if let Some(app_bundle) = runner_layout.app_bundle.as_deref() {
        maybe_sign_output_artifact(app_bundle)?;
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
fn aot_call_conv() -> &'static str {
    if cfg!(windows) {
        "windows_fastcall"
    } else {
        "system_v"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        write_macos_runner_info_plist(&plist, "Chess & TD").expect("write macOS app plist");
        let contents = fs::read_to_string(&plist).expect("read macOS app plist");
        assert!(contents.contains("<key>NSHighResolutionCapable</key>\n    <true/>"));
        assert!(contents.contains("<string>Chess &amp; TD</string>"));
        assert!(contents.contains("<string>org.stasislang.game.chess-td</string>"));
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
        assert_second_file_diagnostic(TargetMode::JitDev);
    }

    #[test]
    fn explicit_project_root_keeps_identity_stable_when_new_directory_is_added() {
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
        assert_second_file_diagnostic(TargetMode::AotProd);
    }

    #[test]
    fn rejected_jit_parse_preserves_accepted_snapshot() {
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
    fn failed_prepared_jit_send_preserves_accepted_snapshot() {
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

    #[test]
    fn identifier_hash_matches_incremental_function() {
        assert_eq!(hash_identifier("on_code_swap"), -663_287_521);
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
                        },
                    ],
                },
            ],
            structs: Vec::new(),
            opaque: Vec::new(),
        };

        let (source, register_lines) =
            build_aot_direct_storage_source(&layout).expect("build enum storage source");

        assert!(source.contains("STASIS_EXPORT int32_t game__phase = 0;"));
        assert!(source.contains("STASIS_EXPORT int32_t game__samples[2] = {0};"));
        assert!(register_lines
            .iter()
            .any(|line| line.contains("stasis_jit_register_global_i32_ptr")));
        assert!(register_lines
            .iter()
            .any(|line| line.contains("stasis_jit_register_global_i32_array")));
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
            },
            PackagedRuntimeField {
                name: "word_values".to_string(),
                size: 4,
                field_type: "u16".to_string(),
                array_count: 2,
                initial_value: Some(serde_json::json!([1, 65535])),
                collection_path: Some("word_values".to_string()),
                collection_field: Some(String::new()),
            },
            PackagedRuntimeField {
                name: "wide_value".to_string(),
                size: 4,
                field_type: "u32".to_string(),
                array_count: 1,
                initial_value: Some(serde_json::json!(4294967295_u64)),
                collection_path: None,
                collection_field: None,
            },
        ];
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &runtime_fields,
            &[],
            &[],
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
            &[],
        )
        .expect("build android bridge source");

        assert!(!source.contains("StasisDirectStorageSlot"));
        assert!(source.contains("STASIS_EXPORT void stasis_init(int width, int height)"));
        assert!(source.contains("host_i32[14] = 3;"));
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
        }];
        let source = build_engine_bundle_runtime_bridge_source(
            &stasis_jit::AotTarget::Native,
            &runtime_fields,
            &["aot_fn_1".to_string()],
            &[PackagedFunctionAlias {
                alias: "main",
                target_symbol: "aot_fn_1".to_string(),
                returns_i32: true,
            }],
            &[],
        )
        .expect("build bridge source");
        assert!(source.contains("typedef signed int int32_t;"));
        assert!(source.contains("int32_t balance__hp[3] = {70, 110, 85};"));
        assert!(!source.starts_with("#include <stdint.h>"));

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
    fn aot_stub_uses_platform_calling_convention() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Literal(7)),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("function %main() -> i32"));
        assert!(clif.contains("iconst.i32 7"));
        assert!(clif.contains(aot_call_conv()));
    }

    #[test]
    fn aot_stub_uses_arithmetic_when_simple_expression_is_available() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Add(
                Box::new(SimpleI32ReturnExpr::Literal(2)),
                Box::new(SimpleI32ReturnExpr::Literal(3)),
            )),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("iconst.i32 2"));
        assert!(clif.contains("iconst.i32 3"));
        assert!(clif.contains("iadd"));
    }

    #[test]
    fn aot_stub_uses_div_and_mod_when_simple_expression_is_available() {
        let div = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Div(
                Box::new(SimpleI32ReturnExpr::Literal(8)),
                Box::new(SimpleI32ReturnExpr::Literal(2)),
            )),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(div.contains("sdiv"));
        let rem = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Mod(
                Box::new(SimpleI32ReturnExpr::Literal(9)),
                Box::new(SimpleI32ReturnExpr::Literal(4)),
            )),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(rem.contains("srem"));
    }

    #[test]
    fn aot_stub_uses_branch_blocks_for_top_level_conditional_expression() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Select(
                SimpleI32Condition::Gt(
                    Box::new(SimpleI32ReturnExpr::Literal(8)),
                    Box::new(SimpleI32ReturnExpr::Literal(3)),
                ),
                Box::new(SimpleI32ReturnExpr::Literal(11)),
                Box::new(SimpleI32ReturnExpr::Literal(22)),
            )),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("icmp sgt"));
        assert!(clif.contains("brif"));
        assert!(clif.contains("block1:"));
        assert!(clif.contains("block2:"));
        assert!(!clif.contains("select "));
    }

    #[test]
    fn aot_stub_uses_void_signature_for_void_return_type() {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            123,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("function %on_code_swap()"));
        assert!(!clif.contains("-> i32"));
        assert!(clif.contains("\nreturn\n"));
        assert!(!clif.contains("iconst.i32"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_for_simple_void_print_metadata() {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(33),
            None,
            None,
            None,
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("iconst.i32 33"));
        assert!(clif.contains("call %print_i32(v0)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_direct_call_target_for_simple_void_metadata() {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("callee_symbol"),
            None,
            None,
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external callee_symbol() -> i32"));
        assert!(clif.contains("v0 = call %callee_symbol()"));
        assert!(clif.contains("call %print_i32(v0)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_direct_call_target_and_add_delta_when_metadata_is_resolved(
    ) {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("callee_symbol"),
            None,
            Some(-4),
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external callee_symbol() -> i32"));
        assert!(clif.contains("v0 = call %callee_symbol()"));
        assert!(clif.contains("iconst.i32 4"));
        assert!(clif.contains("isub v0, v1"));
        assert!(clif.contains("call %print_i32(v2)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_two_call_targets_for_simple_void_metadata() {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            Some("lhs_symbol"),
            Some("rhs_symbol"),
            Some(2),
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external lhs_symbol() -> i32"));
        assert!(clif.contains("external rhs_symbol() -> i32"));
        assert!(clif.contains("v0 = call %lhs_symbol()"));
        assert!(clif.contains("v1 = call %rhs_symbol()"));
        assert!(clif.contains("isub v0, v1"));
        assert!(clif.contains("call %print_i32(v2)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_for_simple_void_metadata() {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(13),
            Some("callee_symbol"),
            None,
            None,
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("iconst.i32 13"));
        assert!(clif.contains("call %callee_symbol(v0)"));
        assert!(clif.contains("call %print_i32(v1)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_for_simple_void_metadata()
    {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("callee_symbol"),
            Some("arg_fn_symbol"),
            None,
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external arg_fn_symbol() -> i32"));
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("v0 = call %arg_fn_symbol()"));
        assert!(clif.contains("v1 = call %callee_symbol(v0)"));
        assert!(clif.contains("call %print_i32(v1)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_and_add_delta_for_simple_void_metadata(
    ) {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(13),
            Some("callee_symbol"),
            None,
            Some(2),
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("iconst.i32 13"));
        assert!(clif.contains("call %callee_symbol(v0)"));
        assert!(clif.contains("iconst.i32 2"));
        assert!(clif.contains("iadd v1, v2"));
        assert!(clif.contains("call %print_i32(v3)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_and_add_delta_for_simple_void_metadata(
    ) {
        let clif = build_aot_stub_clif(
            "on_code_swap",
            "void",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("callee_symbol"),
            Some("arg_fn_symbol"),
            Some(-4),
        );
        assert!(clif.contains("external print_i32(i32)"));
        assert!(clif.contains("external arg_fn_symbol() -> i32"));
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("v0 = call %arg_fn_symbol()"));
        assert!(clif.contains("v1 = call %callee_symbol(v0)"));
        assert!(clif.contains("iconst.i32 4"));
        assert!(clif.contains("isub v1, v2"));
        assert!(clif.contains("call %print_i32(v3)"));
        assert!(clif.contains("\nreturn\n"));
    }

    #[test]
    fn aot_stub_uses_logical_condition_ops_for_select_conditions() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Select(
                SimpleI32Condition::Or(
                    Box::new(SimpleI32Condition::Gt(
                        Box::new(SimpleI32ReturnExpr::Literal(5)),
                        Box::new(SimpleI32ReturnExpr::Literal(2)),
                    )),
                    Box::new(SimpleI32Condition::Not(Box::new(SimpleI32Condition::Eq(
                        Box::new(SimpleI32ReturnExpr::Literal(1)),
                        Box::new(SimpleI32ReturnExpr::Literal(1)),
                    )))),
                ),
                Box::new(SimpleI32ReturnExpr::Literal(9)),
                Box::new(SimpleI32ReturnExpr::Literal(4)),
            )),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("block3:"));
        assert!(clif.matches("brif ").count() >= 2);
        assert!(clif.contains("brif"));
        assert!(!clif.contains("bor "));
        assert!(!clif.contains("bnot "));
    }

    #[test]
    fn aot_stub_uses_short_circuit_branching_for_and_conditions() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            Some(&SimpleI32ReturnExpr::Select(
                SimpleI32Condition::And(
                    Box::new(SimpleI32Condition::Eq(
                        Box::new(SimpleI32ReturnExpr::Literal(1)),
                        Box::new(SimpleI32ReturnExpr::Literal(1)),
                    )),
                    Box::new(SimpleI32Condition::Gt(
                        Box::new(SimpleI32ReturnExpr::Literal(3)),
                        Box::new(SimpleI32ReturnExpr::Literal(2)),
                    )),
                ),
                Box::new(SimpleI32ReturnExpr::Literal(9)),
                Box::new(SimpleI32ReturnExpr::Literal(4)),
            )),
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("block3:"));
        assert!(clif.matches("brif ").count() >= 2);
        assert!(!clif.contains("band "));
    }

    #[test]
    fn aot_stub_uses_direct_call_when_simple_return_call_target_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            Some("callee_symbol"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external callee_symbol() -> i32"));
        assert!(clif.contains("v0 = call %callee_symbol()"));
        assert!(clif.contains("return v0"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_add_delta_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            Some("callee_symbol"),
            Some(5),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external callee_symbol() -> i32"));
        assert!(clif.contains("v0 = call %callee_symbol()"));
        assert!(clif.contains("iconst.i32 5"));
        assert!(clif.contains("iadd v0, v1"));
        assert!(clif.contains("return v2"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_one_i32_arg_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            None,
            None,
            Some("callee_symbol"),
            Some(9),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("iconst.i32 9"));
        assert!(clif.contains("call %callee_symbol(v0)"));
        assert!(clif.contains("return v1"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_first_param_passthrough_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif_for_metric(
            "forward",
            "i32",
            None,
            123,
            None,
            None,
            Some("host_set_summary_file"),
            None,
            None,
            None,
            None,
            None,
            None,
            1,
            0,
            true,
            false,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external host_set_summary_file(i64) -> i32"));
        assert!(clif.contains("function %forward(i64) -> i32"));
        assert!(clif.contains("v1 = call %host_set_summary_file(v0)"));
        assert!(clif.contains("return v1"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_first_second_param_passthrough_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif_for_metric(
            "forward2",
            "i32",
            None,
            123,
            None,
            None,
            None,
            None,
            None,
            Some("host_cli_arg_value"),
            None,
            None,
            None,
            2,
            1,
            false,
            true,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external host_cli_arg_value(i32, i64) -> i32"));
        assert!(clif.contains("function %forward2(i32, i64) -> i32"));
        assert!(clif.contains("v2 = call %host_cli_arg_value(v0, v1)"));
        assert!(clif.contains("return v2"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_first_second_third_param_passthrough_when_metadata_is_resolved(
    ) {
        let clif = build_aot_stub_clif_for_metric(
            "forward3",
            "i32",
            None,
            123,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("host_write_aot_cli_summary"),
            None,
            None,
            3,
            0,
            false,
            false,
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external host_write_aot_cli_summary(i64, i64, i64) -> i32"));
        assert!(clif.contains("function %forward3(i64, i64, i64) -> i32"));
        assert!(clif.contains("v3 = call %host_write_aot_cli_summary(v0, v1, v2)"));
        assert!(clif.contains("return v3"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_first_second_third_fourth_param_passthrough_when_metadata_is_resolved(
    ) {
        let clif = build_aot_stub_clif_for_metric(
            "forward4",
            "i32",
            None,
            123,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("host_load_source_file"),
            None,
            4,
            0,
            false,
            false,
            false,
            true,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external host_load_source_file(i64, i32, i64, i64) -> i32"));
        assert!(clif.contains("function %forward4(i64, i32, i64, i64) -> i32"));
        assert!(clif.contains("v4 = call %host_load_source_file(v0, v1, v2, v3)"));
        assert!(clif.contains("return v4"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_first_second_third_fourth_param_passthrough_add_delta_when_metadata_is_resolved(
    ) {
        let clif = build_aot_stub_clif_for_metric(
            "forward4_add",
            "i32",
            None,
            123,
            None,
            Some(-2),
            None,
            None,
            None,
            None,
            None,
            Some("host_load_source_file"),
            None,
            4,
            0,
            false,
            false,
            false,
            true,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external host_load_source_file(i64, i32, i64, i64) -> i32"));
        assert!(clif.contains("function %forward4_add(i64, i32, i64, i64) -> i32"));
        assert!(clif.contains("v4 = call %host_load_source_file(v0, v1, v2, v3)"));
        assert!(clif.contains("iconst.i32 2"));
        assert!(clif.contains("isub v4, v5"));
        assert!(clif.contains("return v6"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_literal_first_second_param_passthrough_when_metadata_is_resolved(
    ) {
        let clif = build_aot_stub_clif_for_metric(
            "forward_lit",
            "i32",
            None,
            123,
            None,
            None,
            None,
            Some(1),
            None,
            None,
            None,
            None,
            Some("host_cli_arg_value"),
            1,
            0,
            false,
            false,
            false,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external host_cli_arg_value(i32, i64) -> i32"));
        assert!(clif.contains("function %forward_lit(i64) -> i32"));
        assert!(clif.contains("v1 = iconst.i32 1"));
        assert!(clif.contains("v2 = call %host_cli_arg_value(v1, v0)"));
        assert!(clif.contains("return v2"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_one_i32_arg_and_add_delta_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            None,
            Some(2),
            Some("callee_symbol"),
            Some(9),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("iconst.i32 9"));
        assert!(clif.contains("call %callee_symbol(v0)"));
        assert!(clif.contains("iconst.i32 2"));
        assert!(clif.contains("iadd v1, v2"));
        assert!(clif.contains("return v3"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_one_call_arg_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            None,
            None,
            Some("callee_symbol"),
            None,
            Some("arg_fn_symbol"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external arg_fn_symbol() -> i32"));
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("v0 = call %arg_fn_symbol()"));
        assert!(clif.contains("call %callee_symbol(v0)"));
        assert!(clif.contains("return v1"));
    }

    #[test]
    fn aot_stub_uses_direct_call_with_one_call_arg_and_add_delta_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            None,
            Some(-4),
            Some("callee_symbol"),
            None,
            Some("arg_fn_symbol"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external arg_fn_symbol() -> i32"));
        assert!(clif.contains("external callee_symbol(i32) -> i32"));
        assert!(clif.contains("v0 = call %arg_fn_symbol()"));
        assert!(clif.contains("v1 = call %callee_symbol(v0)"));
        assert!(clif.contains("iconst.i32 4"));
        assert!(clif.contains("isub v1, v2"));
        assert!(clif.contains("return v3"));
    }

    #[test]
    fn aot_stub_uses_two_call_add_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            None,
            None,
            None,
            None,
            None,
            Some("lhs_symbol"),
            Some("rhs_symbol"),
            Some(1),
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("external lhs_symbol() -> i32"));
        assert!(clif.contains("external rhs_symbol() -> i32"));
        assert!(clif.contains("v0 = call %lhs_symbol()"));
        assert!(clif.contains("v1 = call %rhs_symbol()"));
        assert!(clif.contains("iadd v0, v1"));
        assert!(clif.contains("return v2"));
        assert!(!clif.contains("iconst.i32 123"));
    }

    #[test]
    fn aot_stub_uses_two_call_sub_when_metadata_is_resolved() {
        let clif = build_aot_stub_clif(
            "main",
            "i32",
            None,
            123,
            None,
            None,
            None,
            None,
            None,
            Some("lhs_symbol"),
            Some("rhs_symbol"),
            Some(2),
            None,
            None,
            None,
            None,
        );
        assert!(clif.contains("isub v0, v1"));
    }

    #[test]
    fn resolve_simple_i32_return_call_target_symbol_returns_unique_match() {
        let target_id_hash = hash_identifier("callee");
        let caller = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 0,
            id_hash: hash_identifier("main"),
            sig_hash: 11,
            body_hash: 12,
            return_type_code: stasis_compiler::RETURN_TYPE_CODE_I32,
            param_count: 0,
            first_param_type_code: 0,
            clif_text: String::new(),
        };
        let callee = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 1,
            id_hash: hash_identifier("callee"),
            sig_hash: 21,
            body_hash: 22,
            return_type_code: stasis_compiler::RETURN_TYPE_CODE_I32,
            param_count: 0,
            first_param_type_code: 0,
            clif_text: String::new(),
        };
        let metrics = vec![caller.clone(), callee.clone()];
        let resolved =
            resolve_unique_i32_call_target_symbol_by_hash(Some(target_id_hash), &metrics)
                .expect("resolved");
        assert_eq!(resolved, aot_symbol_name(&callee));
    }

    #[test]
    fn resolve_simple_i32_return_call_target_symbol_rejects_one_arg_candidate_for_noarg_call() {
        let target_id_hash = hash_identifier("callee");
        let caller = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 0,
            id_hash: hash_identifier("main"),
            sig_hash: 11,
            body_hash: 12,
            return_type_code: stasis_compiler::RETURN_TYPE_CODE_I32,
            param_count: 0,
            first_param_type_code: 0,
            clif_text: String::new(),
        };
        let callee = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 1,
            id_hash: hash_identifier("callee"),
            sig_hash: 21,
            body_hash: 22,
            return_type_code: stasis_compiler::RETURN_TYPE_CODE_I32,
            param_count: 1,
            first_param_type_code: 1,
            clif_text: String::new(),
        };
        let metrics = vec![caller.clone(), callee];
        let resolved =
            resolve_unique_i32_call_target_symbol_by_hash(Some(target_id_hash), &metrics);
        assert!(
            resolved.is_none(),
            "no-arg call resolution should reject one-arg candidate signature"
        );
    }

    #[test]
    fn resolve_simple_i32_return_call_target_symbol_supports_known_host_noarg_extern() {
        let resolved = resolve_unique_i32_call_target_symbol_by_hash(
            Some(hash_identifier("host_cli_arg_count")),
            &[],
        )
        .expect("known host extern should resolve");
        assert_eq!(resolved, "host_cli_arg_count");
        let resolved_runtime_entry = resolve_unique_i32_call_target_symbol_by_hash(
            Some(hash_identifier("host_run_self_host_aot_cli_from_env")),
            &[],
        )
        .expect("known host runtime entry extern should resolve");
        assert_eq!(
            resolved_runtime_entry,
            "host_run_self_host_aot_cli_from_env"
        );
    }

    #[test]
    fn resolve_simple_i32_return_one_arg_target_symbol_supports_known_host_single_arg_extern() {
        let summary = resolve_unique_i32_single_arg_call_target_symbol_by_hash(
            Some(hash_identifier("host_set_summary_file")),
            &[],
            0,
        )
        .expect("known host summary extern should resolve");
        assert_eq!(summary, "host_set_summary_file");
        let source_count = resolve_unique_i32_single_arg_call_target_symbol_by_hash(
            Some(hash_identifier("host_source_file_count")),
            &[],
            0,
        )
        .expect("known host source-count extern should resolve");
        assert_eq!(source_count, "host_source_file_count");
    }

    #[test]
    fn resolve_simple_i32_return_two_arg_passthrough_target_symbol_supports_known_host_extern() {
        let resolved = resolve_known_host_two_arg_i32_extern_symbol_by_hash(
            Some(hash_identifier("host_cli_arg_value")),
            1,
        )
        .expect("known host cli arg-value extern should resolve");
        assert_eq!(resolved, "host_cli_arg_value");
    }

    #[test]
    fn resolve_simple_i32_return_three_arg_passthrough_target_symbol_supports_known_host_extern() {
        let resolved = resolve_known_host_three_arg_i32_extern_symbol_by_hash(
            Some(hash_identifier("host_write_aot_cli_summary")),
            0,
        )
        .expect("known host aot summary extern should resolve");
        assert_eq!(resolved, "host_write_aot_cli_summary");
    }

    #[test]
    fn resolve_simple_i32_return_four_arg_passthrough_target_symbol_supports_known_host_extern() {
        let resolved = resolve_known_host_four_arg_i32_extern_symbol_by_hash(
            Some(hash_identifier("host_load_source_file")),
            0,
        )
        .expect("known host source-loader extern should resolve");
        assert_eq!(resolved, "host_load_source_file");
    }

    #[test]
    fn resolve_simple_i32_return_two_arg_literal_first_second_param_passthrough_target_symbol_supports_known_host_extern(
    ) {
        let resolved =
            resolve_known_host_two_arg_literal_first_second_param_i32_extern_symbol_by_hash(
                Some(hash_identifier("host_cli_arg_value")),
                0,
            )
            .expect("known host cli arg-value literal+param extern should resolve");
        assert_eq!(resolved, "host_cli_arg_value");
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
        if !std::env::var("STASIS_AOT_QUALITY_GATE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        {
            return;
        }

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
        fn find_lld_link() -> Option<PathBuf> {
            let candidates = [
                r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\lld-link.exe",
                r"C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\lld-link.exe",
                r"C:\Program Files (x86)\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin\lld-link.exe",
                r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\Llvm\x64\bin\lld-link.exe",
            ];
            for candidate in candidates {
                let path = PathBuf::from(candidate);
                if path.exists() {
                    return Some(path);
                }
            }
            if std::process::Command::new("lld-link.exe")
                .arg("/NOLOGO")
                .output()
                .is_ok()
            {
                return Some(PathBuf::from("lld-link.exe"));
            }
            None
        }

        fn hash_global_path(path: &str) -> i32 {
            // Must match `crates/stasis_compiler/src/backend/jit.rs::hash_global_path`.
            let mut hash: u32 = 2166136261;
            for byte in path.bytes() {
                hash ^= u32::from(byte);
                hash = hash.wrapping_mul(16777619);
            }
            hash as i32
        }

        fn resolve_stasis_dynload_lib() -> Option<PathBuf> {
            let target_dir = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("..")
                        .join("..")
                        .join("target")
                });
            let mut candidates: Vec<PathBuf> = Vec::new();

            for profile in ["debug", "release"] {
                let base = target_dir.join(profile);
                let direct = base.join("stasis_dynload.lib");
                if direct.exists() {
                    candidates.push(direct);
                }

                let deps = base.join("deps");
                let Ok(entries) = fs::read_dir(&deps) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                        continue;
                    };
                    if !name.starts_with("stasis_dynload-") || !name.ends_with(".lib") {
                        continue;
                    }
                    candidates.push(path);
                }
            }

            candidates.into_iter().max_by_key(|path| {
                fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .unwrap_or_default()
            })
        }

        if !std::env::var("STASIS_AOT_QUALITY_GATE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        {
            return;
        }

        // Compile/link steps can consult/modify process-wide env; serialize with other tests.
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");

        // Ensure the `stasis_dynload` staticlib exists and is up-to-date before linking.
        let mut dynload_build_command = Command::new("cargo");
        dynload_build_command
            .arg("rustc")
            .arg("-p")
            .arg("stasis_dynload");
        if !cfg!(debug_assertions) {
            dynload_build_command.arg("--release");
        }
        dynload_build_command
            .arg("--")
            .arg("--crate-type")
            .arg("staticlib")
            .current_dir(&repo_root);
        if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
            dynload_build_command.env("CARGO_TARGET_DIR", target_dir);
        }
        let dynload_build = dynload_build_command
            .output()
            .expect("spawn cargo rustc -p stasis_dynload --crate-type staticlib");
        assert!(
            dynload_build.status.success(),
            "failed to build stasis_dynload staticlib\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&dynload_build.stdout),
            String::from_utf8_lossy(&dynload_build.stderr)
        );

        let linker_path = find_lld_link().expect("lld-link.exe required for AOT quality gate");
        let stasis_dynload_lib = resolve_stasis_dynload_lib()
            .expect("stasis_dynload staticlib required for AOT quality gate");

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
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_brickout_exec_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let artifact_root = temp_root.join("aot_artifacts");

        let compile_config = AotCompileConfig::default();
        assert_eq!(
            compile_config.opt_level.as_str(),
            "speed_and_size",
            "AOT compile config default opt_level should be speed_and_size for release-like engine bundles"
        );
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(compile_config, artifact_root);
        let result = backend.compile(CompileRequest::new(
            RequestId(9_202),
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
        let manifest = backend
            .read_engine_bundle_manifest(&bundle.manifest_path)
            .expect("read engine bundle manifest");
        assert_eq!(
            manifest.optimization_profile.as_deref(),
            Some("speed_and_size"),
            "engine bundle manifest should report speed_and_size optimization by default"
        );

        let main_symbol = manifest
            .functions
            .iter()
            .find(|row| row.name == "main")
            .map(|row| row.symbol.clone())
            .expect("manifest should include main");
        let tick_symbol = manifest
            .functions
            .iter()
            .find(|row| row.name == "tick")
            .map(|row| row.symbol.clone())
            .expect("manifest should include tick");

        let object_paths: Vec<PathBuf> = bundle.object_paths().cloned().collect();
        assert!(
            !object_paths.is_empty(),
            "expected engine bundle to include object files"
        );

        let linked_output = temp_root.join("brickout_aot_bundle.dll");
        let export_symbols = vec![
            main_symbol.clone(),
            tick_symbol.clone(),
            // Seed host frame values in the same runtime instance as the linked AOT code.
            "stasis_jit_global_i32_array_store".to_string(),
        ];
        let link_config = stasis_jit::AotLinkConfig {
            linker_path: Some(linker_path),
            runtime_lib_paths: vec![stasis_dynload_lib],
            target: stasis_jit::AotTarget::default(),
        };

        stasis_jit::link_objects_to_dynamic_library(
            &object_paths,
            &linked_output,
            &export_symbols,
            &link_config,
        )
        .expect("link engine bundle into dll");

        let library = stasis_dynload::Library::load(&linked_output).expect("load linked image");
        let main_ptr = library
            .symbol_address(&main_symbol)
            .expect("resolve main export");
        let tick_ptr = library
            .symbol_address(&tick_symbol)
            .expect("resolve tick export");

        let store_ptr = library
            .symbol_address("stasis_jit_global_i32_array_store")
            .expect("resolve host_i32 store");
        let store_f32_ptr = library
            .symbol_address("stasis_jit_global_f32_array_store")
            .expect("resolve host_f32 store");

        let host_i32 = hash_global_path("host_i32");
        let host_f32 = hash_global_path("host_f32");
        let field = 0;
        let store = |index: i32, value: i32| {
            stasis_dynload::invoke_i32_i32_i32_i32_to_void(
                store_ptr, host_i32, field, index, value,
            )
            .expect("invoke host_i32 store");
        };
        let store_f32 = |index: i32, value: f32| {
            stasis_dynload::invoke_i32_i32_i32_f32_to_void(
                store_f32_ptr,
                host_f32,
                field,
                index,
                value,
            )
            .expect("invoke host_f32 store");
        };

        // Seed enough HostFrame state for Brickout to initialize and tick headlessly.
        // Indices from src/stdlib/internal/host_frame.stasis.
        let t0_ms: i32 = 12345;
        store(0, t0_ms); // HOST_I_TIME_MS
        store_f32(50, 360.0); // HOST_F_LOGICAL_W
        store_f32(51, 720.0); // HOST_F_LOGICAL_H
        store_f32(52, 0.0); // HOST_F_SAFE_X
        store_f32(53, 0.0); // HOST_F_SAFE_Y
        store_f32(54, 360.0); // HOST_F_SAFE_W
        store_f32(55, 720.0); // HOST_F_SAFE_H
        store(7, 0); // HOST_I_POINTER_COUNT
        store(8, 0); // HOST_I_DROPPED_POINTERS
        store(9, 0); // HOST_I_QUIT_REQUESTED
        store(10, 0); // HOST_I_TICK_INDEX
        store(11, 1); // HOST_I_RESIZED
        store(12, 360); // HOST_I_SCREEN_W_PX
        store(13, 720); // HOST_I_SCREEN_H_PX
        store(19, t0_ms * 1000); // HOST_I_TIME_US (coarse is fine)

        let main_rc = stasis_dynload::invoke_noarg_i32(main_ptr).expect("invoke main");
        assert_eq!(main_rc, 0, "expected Brickout main() to succeed");

        // Clear resize flag for subsequent ticks.
        store(11, 0);

        for tick_index in 0..2 {
            let time_ms = t0_ms + (tick_index + 1) * 16;
            store(0, time_ms);
            store(10, tick_index);
            store(19, time_ms * 1000);

            let rc = stasis_dynload::invoke_noarg_i32(tick_ptr).expect("invoke tick");
            assert_eq!(rc, 0, "expected tick() to return 0 (keep running)");
        }

        drop(library);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn jit_dev_engine_mode_rebuilds_one_complete_generation_between_compiles() {
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

    #[test]
    fn aot_compile_rejects_unresolved_direct_call_target() {
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
                diagnostic.message.contains("call target") && diagnostic.message.contains("callee")
            }),
            "expected unresolved direct call target diagnostic, got: {:?}",
            result.diagnostics
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_direct_call_target_without_fallback() {
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
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(manifest.fallback_stub_symbols.is_empty());

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_writes_manifest_with_artifacts_on_success() {
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
            &[],
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
        let Some(linker_path) = find_lld_link() else {
            return;
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_bounded_performance_aot_{stamp}"));
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
            &[],
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
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = SIGN_ENV_LOCK.lock().expect("lock signer env");
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

        result.expect("self-host signing run should succeed");
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
    fn self_host_aot_cli_writes_default_summary_sidecar() {
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
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let old_signer = std::env::var("STASIS_AOT_SIGN_TOOL").ok();
        std::env::remove_var("STASIS_AOT_SIGN_TOOL");
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
        if let Some(value) = old_signer {
            std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
        } else {
            std::env::remove_var("STASIS_AOT_SIGN_TOOL");
        }
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
    let mut candidate = backend.compile_aot_process_from_source_cache()?;
    let function_entries = snapshot_function_entries(
        candidate
            .program_snapshot()
            .expect("compiled self-host AOT candidate snapshot"),
    );
    let include_on_code_swap = function_entries
        .iter()
        .any(|entry| entry.name == "on_code_swap");
    let use_engine_mode_contracts = function_entries.iter().any(|entry| entry.name == "tick")
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
            &IncrementalCompilerBackend::engine_entrypoints(include_on_code_swap),
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
    let output_key = output_exe
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("aot_output");
    let artifact_root = project_dir
        .join(".stasis_cache")
        .join("aot_cli")
        .join(output_key);
    let mut backend = IncrementalCompilerBackend::new_self_host_aot_cli(artifact_root);
    let options = SelfHostedAotCliOptions::new(
        summary_file_path.map(PathBuf::from),
        entry_file.map(PathBuf::from),
    );
    let mut summary = run_self_host_aot_cli_with_backend_and_options(
        &mut backend,
        project_dir,
        output_exe,
        &options,
    )?;
    summary.program_snapshot = backend.last_program_snapshot.clone();
    Ok(summary)
}

pub fn run_self_host_aot_cli(
    project_dir: &Path,
    output_exe: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    run_self_host_aot_cli_with_options(project_dir, output_exe, None, None)
}

fn maybe_sign_output_artifact(artifact_path: &Path) -> Result<(), String> {
    let Some(sign_tool) = std::env::var_os("STASIS_AOT_SIGN_TOOL") else {
        return Ok(());
    };
    let status = std::process::Command::new(&sign_tool)
        .arg(artifact_path)
        .status()
        .map_err(|error| {
            format!(
                "failed to launch signer tool {:?} for {}: {error}",
                sign_tool,
                artifact_path.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "signer tool {:?} failed for {} with status {:?}",
            sign_tool,
            artifact_path.display(),
            status.code()
        ));
    }
    Ok(())
}

fn maybe_sign_output_executable(output_exe: &Path) -> Result<(), String> {
    maybe_sign_output_artifact(output_exe)
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
