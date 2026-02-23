use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stasis_compiler::{IncrementalCompilerHost, SimpleI32Condition, SimpleI32ReturnExpr};
use stasis_jit::{
    compile_clif_to_object, link_objects_to_dynamic_library, link_objects_to_executable,
    AotCompileConfig, AotLinkConfig,
};
use stasis_runner::swap::contracts::{
    AotFunctionSymbol, CompileRequest, CompileResult, Diagnostic, DiagnosticSeverity, FnId,
    FunctionPatch, FunctionPatchSet, LayoutHash, RequestId, TargetMode,
};
use stasis_runner::swap::pipeline::CompilerBackend;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct IncrementalCompilerBackend {
    host: IncrementalCompilerHost,
    fn_id_by_signature: BTreeMap<String, FnId>,
    next_fn_id: u32,
    aot_compile_config: AotCompileConfig,
    aot_link_config: AotLinkConfig,
    aot_artifact_root: std::path::PathBuf,
    enable_aot_link_step: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHostedAotCliSummary {
    pub source_file_count: usize,
    pub linked_image_path: PathBuf,
    pub entry_symbol: String,
    pub ir_bundle_path: PathBuf,
    pub object_bundle_path: PathBuf,
    pub object_file_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AotFallbackStubDetail {
    symbol: String,
    id_hash: i32,
    sig_hash: i32,
    body_hash: i32,
    ordinal: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AotPatchManifest {
    request_id: u64,
    artifact_paths: Vec<String>,
    linked_image_path: Option<String>,
    linked_image_size_bytes: Option<u64>,
    linked_image_sha256: Option<String>,
    #[serde(default)]
    fallback_stub_symbols: Vec<String>,
    #[serde(default)]
    fallback_stub_details: Vec<AotFallbackStubDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SelfHostIrBundle {
    source_file_count: usize,
    entry_symbol: String,
    object_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SelfHostObjectBundle {
    entry_symbol: String,
    object_paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct SelfHostCliEnvSnapshot {
    strict_self_host: bool,
    allow_stub_fallback: bool,
    quality_gate: bool,
    summary_file_path: Option<PathBuf>,
}

fn capture_self_host_cli_env_snapshot() -> SelfHostCliEnvSnapshot {
    SelfHostCliEnvSnapshot {
        strict_self_host: std::env::var("STASIS_AOT_STRICT_SELF_HOST")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
        allow_stub_fallback: std::env::var("STASIS_AOT_ALLOW_STUB_FALLBACK")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
        quality_gate: std::env::var("STASIS_AOT_QUALITY_GATE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
        summary_file_path: std::env::var_os("STASIS_AOT_SUMMARY_FILE")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from),
    }
}

impl IncrementalCompilerBackend {
    pub fn new() -> Self {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        Self {
            host: IncrementalCompilerHost::new(),
            fn_id_by_signature: BTreeMap::new(),
            next_fn_id: 1,
            aot_compile_config: AotCompileConfig::default(),
            aot_link_config: AotLinkConfig::default(),
            aot_artifact_root: repo_root.join(".stasis_cache").join("aot"),
            enable_aot_link_step: std::env::var("STASIS_AOT_LINK_ARTIFACTS")
                .ok()
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
        }
    }

    pub fn new_self_host_aot_cli(aot_artifact_root: PathBuf) -> Self {
        let mut backend = Self::new();
        backend.aot_artifact_root = aot_artifact_root;
        backend.enable_aot_link_step = false;
        backend
    }

    #[cfg(test)]
    fn with_aot_config(
        aot_compile_config: AotCompileConfig,
        aot_artifact_root: std::path::PathBuf,
    ) -> Self {
        Self {
            host: IncrementalCompilerHost::new(),
            fn_id_by_signature: BTreeMap::new(),
            next_fn_id: 1,
            aot_compile_config,
            aot_link_config: AotLinkConfig::default(),
            aot_artifact_root,
            enable_aot_link_step: false,
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
            host: IncrementalCompilerHost::new(),
            fn_id_by_signature: BTreeMap::new(),
            next_fn_id: 1,
            aot_compile_config,
            aot_link_config,
            aot_artifact_root,
            enable_aot_link_step,
        }
    }

    fn fn_id_for_key(&mut self, key: &str) -> Result<FnId, String> {
        if let Some(existing) = self.fn_id_by_signature.get(key) {
            return Ok(*existing);
        }
        if self.next_fn_id == u32::MAX {
            return Err("function id space exhausted".to_string());
        }
        let next = FnId(self.next_fn_id);
        self.next_fn_id += 1;
        self.fn_id_by_signature.insert(key.to_string(), next);
        Ok(next)
    }
}

impl Default for IncrementalCompilerBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CompilerBackend for IncrementalCompilerBackend {
    fn compile(&mut self, request: CompileRequest) -> CompileResult {
        let parsed = match self.host.compile_changed_files(&request.changed_files) {
            Ok(result) => result,
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

        if parsed.status != 0 {
            let mut diagnostics = Vec::new();
            if parsed.errors.is_empty() {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: format!("incremental compiler failed with status {}", parsed.status),
                    path: request.changed_files.first().cloned(),
                    line: None,
                    column: None,
                });
            } else {
                for error in parsed.errors {
                    diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        message: format_error_message(error.code, error.detail_a, error.detail_b),
                        path: None,
                        line: None,
                        column: Some(error.pos.max(0) as u32),
                    });
                }
            }
            return CompileResult::failed(request.request_id, diagnostics);
        }

        let mut aot_linked_image_path: Option<String> = None;
        let mut aot_linked_image_size_bytes: Option<u64> = None;
        let mut aot_linked_image_sha256: Option<String> = None;
        let mut aot_function_symbols: Option<Vec<AotFunctionSymbol>> = None;
        if request.target_mode == TargetMode::AotProd {
            match self.emit_aot_artifacts(request.request_id.0, &parsed.functions) {
                Ok(path) => {
                    aot_linked_image_path = path;
                    if let Some(path) = aot_linked_image_path.as_ref() {
                        let metadata = std::fs::metadata(path).map_err(|error| {
                            CompileResult::failed(
                                request.request_id,
                                vec![Diagnostic {
                                    severity: DiagnosticSeverity::Error,
                                    message: format!(
                                        "failed to stat linked AOT image {}: {error}",
                                        path
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
                        let digest = compute_file_sha256_hex(Path::new(path)).map_err(|error| {
                            CompileResult::failed(
                                request.request_id,
                                vec![Diagnostic {
                                    severity: DiagnosticSeverity::Error,
                                    message: format!(
                                        "failed to hash linked AOT image {}: {error}",
                                        path
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
                    }
                }
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
            }
        }

        let mut functions = Vec::new();
        let mut hook_fn_id: Option<FnId> = None;
        let mut collected_aot_symbols: Vec<AotFunctionSymbol> = Vec::new();
        for metric in parsed.functions {
            let path = parsed
                .file_paths
                .get(metric.file_index)
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());
            let key = format!(
                "{path}::{}::{}::{}",
                metric.id_hash, metric.sig_hash, metric.body_hash
            );
            let fn_id = match self.fn_id_for_key(&key) {
                Ok(id) => id,
                Err(message) => {
                    return CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            message,
                            path: None,
                            line: None,
                            column: None,
                        }],
                    );
                }
            };
            if metric.id_hash == hash_identifier("on_code_swap") {
                hook_fn_id = Some(fn_id);
            }
            if request.target_mode == TargetMode::AotProd {
                collected_aot_symbols.push(AotFunctionSymbol {
                    fn_id,
                    symbol: aot_symbol_name(&metric),
                });
            }
            functions.push(FunctionPatch { fn_id });
        }
        if request.target_mode == TargetMode::AotProd {
            aot_function_symbols = Some(collected_aot_symbols);
        }

        if hook_fn_id.is_none() {
            hook_fn_id = self.existing_fn_id_for_identifier_hash(hash_identifier("on_code_swap"));
        }

        let layout_hash = expand_layout_hash(parsed.layout_hash);
        CompileResult::success_with_host_set_metadata(
            request.request_id,
            layout_hash,
            FunctionPatchSet { functions },
            request.host_set_id.clone(),
            request.host_set_hash,
            parsed.hook_symbol,
            hook_fn_id,
            aot_linked_image_path.map(std::path::PathBuf::from),
            aot_linked_image_size_bytes,
            aot_linked_image_sha256,
            aot_function_symbols,
        )
    }
}

impl IncrementalCompilerBackend {
    fn existing_fn_id_for_identifier_hash(&self, id_hash: i32) -> Option<FnId> {
        let token = format!("::{id_hash}::");
        self.fn_id_by_signature
            .iter()
            .find_map(|(key, fn_id)| key.contains(&token).then_some(*fn_id))
    }

    fn emit_aot_artifacts(
        &self,
        request_id: u64,
        metrics: &[stasis_compiler::FunctionMetric],
    ) -> Result<Option<String>, String> {
        let mut artifact_paths = Vec::new();
        let mut export_symbols = Vec::new();
        let mut fallback_stub_symbols = Vec::new();
        let mut fallback_stub_details = Vec::new();
        let mut linked_image_path: Option<String> = None;
        let mut linked_image_size_bytes: Option<u64> = None;
        let mut linked_image_sha256: Option<String> = None;
        if metrics.is_empty() {
            self.write_aot_manifest(
                request_id,
                &artifact_paths,
                linked_image_path.as_deref(),
                linked_image_size_bytes,
                linked_image_sha256.as_deref(),
                &fallback_stub_symbols,
                &fallback_stub_details,
            )?;
            return Ok(linked_image_path);
        }

        std::fs::create_dir_all(&self.aot_artifact_root).map_err(|error| {
            format!(
                "failed to create AOT artifact directory {}: {error}",
                self.aot_artifact_root.display()
            )
        })?;

        for metric in metrics {
            let function_name = aot_symbol_name(metric);
            export_symbols.push(function_name.clone());
            let uses_stub_fallback = metric_uses_stub_fallback(metric);
            if uses_stub_fallback {
                fallback_stub_symbols.push(function_name.clone());
                fallback_stub_details.push(AotFallbackStubDetail {
                    symbol: function_name.clone(),
                    id_hash: metric.id_hash,
                    sig_hash: metric.sig_hash,
                    body_hash: metric.body_hash,
                    ordinal: metric.ordinal,
                });
            }
            let resolved_simple_call_target = resolve_unique_i32_call_target_symbol_by_hash(
                metric.simple_i32_return_call_target_id_hash,
                metrics,
            );
            if metric.simple_i32_return_call_target_id_hash.is_some()
                && resolved_simple_call_target.is_none()
            {
                return Err(format!(
                    "unresolved direct call target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            let simple_i32_one_arg_uses_first_param_passthrough = metric
                .simple_i32_return_call_one_arg_target_id_hash
                .is_some()
                && metric.simple_i32_return_call_one_arg_i32_literal.is_none()
                && metric
                    .simple_i32_return_call_one_arg_arg_call_target_id_hash
                    .is_none()
                && metric.param_count == 1;
            let simple_i32_two_arg_uses_first_second_param_passthrough = metric
                .simple_i32_return_call_one_arg_target_id_hash
                .is_some()
                && metric.simple_i32_return_call_one_arg_i32_literal.is_none()
                && metric
                    .simple_i32_return_call_one_arg_arg_call_target_id_hash
                    .is_none()
                && metric.param_count == 2;
            let simple_i32_three_arg_uses_first_second_third_param_passthrough = metric
                .simple_i32_return_call_one_arg_target_id_hash
                .is_some()
                && metric.simple_i32_return_call_one_arg_i32_literal.is_none()
                && metric
                    .simple_i32_return_call_one_arg_arg_call_target_id_hash
                    .is_none()
                && metric.param_count == 3;
            let simple_i32_four_arg_uses_first_second_third_fourth_param_passthrough = metric
                .simple_i32_return_call_one_arg_target_id_hash
                .is_some()
                && metric.simple_i32_return_call_one_arg_i32_literal.is_none()
                && metric
                    .simple_i32_return_call_one_arg_arg_call_target_id_hash
                    .is_none()
                && metric.param_count == 4;
            let simple_i32_two_arg_uses_literal_first_second_param_passthrough = metric
                .simple_i32_return_call_one_arg_target_id_hash
                .is_some()
                && metric.simple_i32_return_call_one_arg_i32_literal.is_some()
                && metric
                    .simple_i32_return_call_one_arg_arg_call_target_id_hash
                    .is_none()
                && metric.param_count == 1;
            let resolved_simple_two_arg_passthrough_call_target = if simple_i32_two_arg_uses_first_second_param_passthrough {
                resolve_known_host_two_arg_i32_extern_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_target_id_hash,
                    metric.first_param_type_code,
                )
            } else {
                None
            };
            let resolved_simple_three_arg_passthrough_call_target = if simple_i32_three_arg_uses_first_second_third_param_passthrough {
                resolve_known_host_three_arg_i32_extern_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_target_id_hash,
                    metric.first_param_type_code,
                )
            } else {
                None
            };
            let resolved_simple_four_arg_passthrough_call_target = if simple_i32_four_arg_uses_first_second_third_fourth_param_passthrough {
                resolve_known_host_four_arg_i32_extern_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_target_id_hash,
                    metric.first_param_type_code,
                )
            } else {
                None
            };
            let resolved_simple_two_arg_literal_first_second_passthrough_call_target = if simple_i32_two_arg_uses_literal_first_second_param_passthrough {
                resolve_known_host_two_arg_literal_first_second_param_i32_extern_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_target_id_hash,
                    metric.first_param_type_code,
                )
            } else {
                None
            };
            let resolved_simple_one_arg_call_target = if simple_i32_two_arg_uses_first_second_param_passthrough
                || simple_i32_three_arg_uses_first_second_third_param_passthrough
                || simple_i32_four_arg_uses_first_second_third_fourth_param_passthrough
                || simple_i32_two_arg_uses_literal_first_second_param_passthrough
            {
                None
            } else {
                resolve_unique_i32_single_arg_call_target_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_target_id_hash,
                    metrics,
                    if simple_i32_one_arg_uses_first_param_passthrough {
                        metric.first_param_type_code
                    } else {
                        1
                    },
                )
            };
            if metric.simple_i32_return_call_one_arg_target_id_hash.is_some() {
                if simple_i32_four_arg_uses_first_second_third_fourth_param_passthrough {
                    if resolved_simple_four_arg_passthrough_call_target.is_none() {
                        return Err(format!(
                            "unresolved four-arg passthrough direct call target for emitted function {} (id_hash={})",
                            function_name, metric.id_hash
                        ));
                    }
                } else if simple_i32_two_arg_uses_literal_first_second_param_passthrough {
                    if resolved_simple_two_arg_literal_first_second_passthrough_call_target
                        .is_none()
                    {
                        return Err(format!(
                            "unresolved two-arg literal+param passthrough direct call target for emitted function {} (id_hash={})",
                            function_name, metric.id_hash
                        ));
                    }
                } else if simple_i32_three_arg_uses_first_second_third_param_passthrough {
                    if resolved_simple_three_arg_passthrough_call_target.is_none() {
                        return Err(format!(
                            "unresolved three-arg passthrough direct call target for emitted function {} (id_hash={})",
                            function_name, metric.id_hash
                        ));
                    }
                } else if simple_i32_two_arg_uses_first_second_param_passthrough {
                    if resolved_simple_two_arg_passthrough_call_target.is_none() {
                        return Err(format!(
                            "unresolved two-arg passthrough direct call target for emitted function {} (id_hash={})",
                            function_name, metric.id_hash
                        ));
                    }
                } else if resolved_simple_one_arg_call_target.is_none() {
                    return Err(format!(
                        "unresolved one-arg direct call target for emitted function {} (id_hash={})",
                        function_name, metric.id_hash
                    ));
                }
            }
            let resolved_simple_one_arg_arg_call_target =
                resolve_unique_i32_call_target_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_arg_call_target_id_hash,
                    metrics,
                );
            if metric
                .simple_i32_return_call_one_arg_arg_call_target_id_hash
                .is_some()
                && resolved_simple_one_arg_arg_call_target.is_none()
            {
                return Err(format!(
                    "unresolved one-arg direct call argument target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            let simple_void_print_is_one_arg =
                metric.simple_void_print_i32_call_target_id_hash.is_some()
                    && metric.simple_void_print_i32_literal.is_some();
            let resolved_simple_void_print_one_arg_arg_call_target =
                resolve_unique_i32_call_target_symbol_by_hash(
                    metric.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
                    metrics,
                );
            if metric
                .simple_void_print_i32_call_one_arg_arg_call_target_id_hash
                .is_some()
                && resolved_simple_void_print_one_arg_arg_call_target.is_none()
            {
                return Err(format!(
                    "unresolved void print_i32 one-arg argument target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            let resolved_simple_void_print_call_target = if simple_void_print_is_one_arg {
                resolve_unique_i32_single_arg_call_target_symbol_by_hash(
                    metric.simple_void_print_i32_call_target_id_hash,
                    metrics,
                    1,
                )
            } else if metric
                .simple_void_print_i32_call_one_arg_arg_call_target_id_hash
                .is_some()
            {
                resolve_unique_i32_single_arg_call_target_symbol_by_hash(
                    metric.simple_void_print_i32_call_target_id_hash,
                    metrics,
                    1,
                )
            } else {
                resolve_unique_i32_call_target_symbol_by_hash(
                    metric.simple_void_print_i32_call_target_id_hash,
                    metrics,
                )
            };
            if metric.simple_void_print_i32_call_target_id_hash.is_some()
                && resolved_simple_void_print_call_target.is_none()
            {
                if simple_void_print_is_one_arg {
                    return Err(format!(
                        "unresolved void print_i32 one-arg call target for emitted function {} (id_hash={})",
                        function_name, metric.id_hash
                    ));
                }
                return Err(format!(
                    "unresolved void print_i32 call target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            let resolved_simple_two_call_left_target =
                resolve_unique_i32_call_target_symbol_by_hash(
                    metric.simple_i32_return_two_call_left_target_id_hash,
                    metrics,
                );
            let resolved_simple_two_call_right_target =
                resolve_unique_i32_call_target_symbol_by_hash(
                    metric.simple_i32_return_two_call_right_target_id_hash,
                    metrics,
                );
            if metric
                .simple_i32_return_two_call_left_target_id_hash
                .is_some()
                && resolved_simple_two_call_left_target.is_none()
            {
                return Err(format!(
                    "unresolved two-call left target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            if metric
                .simple_i32_return_two_call_right_target_id_hash
                .is_some()
                && resolved_simple_two_call_right_target.is_none()
            {
                return Err(format!(
                    "unresolved two-call right target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            let object_path = self.aot_artifact_root.join(format!(
                "req{}_f{}_{}.o",
                request_id, metric.file_index, metric.ordinal
            ));
            if metric.clif_text.is_empty() {
                return Err(format!(
                    "missing stasis-emitted clif text for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            compile_clif_to_object(&metric.clif_text, &object_path, &self.aot_compile_config)?;
            artifact_paths.push(object_path.display().to_string());
        }
        if self.enable_aot_link_step {
            let linked_output = if cfg!(windows) {
                self.aot_artifact_root
                    .join(format!("req{request_id}_bundle.dll"))
            } else if cfg!(target_os = "macos") {
                self.aot_artifact_root
                    .join(format!("req{request_id}_bundle.dylib"))
            } else {
                self.aot_artifact_root
                    .join(format!("req{request_id}_bundle.so"))
            };
            let object_paths: Vec<std::path::PathBuf> = artifact_paths
                .iter()
                .map(std::path::PathBuf::from)
                .collect();
            link_objects_to_dynamic_library(
                &object_paths,
                &linked_output,
                &export_symbols,
                &self.aot_link_config,
            )?;
            linked_image_path = Some(linked_output.display().to_string());
            linked_image_size_bytes = Some(
                std::fs::metadata(&linked_output)
                    .map_err(|error| {
                        format!(
                            "failed to stat linked AOT image {}: {error}",
                            linked_output.display()
                        )
                    })?
                    .len(),
            );
            linked_image_sha256 = Some(compute_file_sha256_hex(&linked_output)?);
        }
        self.write_aot_manifest(
            request_id,
            &artifact_paths,
            linked_image_path.as_deref(),
            linked_image_size_bytes,
            linked_image_sha256.as_deref(),
            &fallback_stub_symbols,
            &fallback_stub_details,
        )?;

        Ok(linked_image_path)
    }

    fn write_aot_manifest(
        &self,
        request_id: u64,
        artifact_paths: &[String],
        linked_image_path: Option<&str>,
        linked_image_size_bytes: Option<u64>,
        linked_image_sha256: Option<&str>,
        fallback_stub_symbols: &[String],
        fallback_stub_details: &[AotFallbackStubDetail],
    ) -> Result<(), String> {
        let manifest_path = self.aot_artifact_root.join("last_patch_manifest.json");
        let manifest = AotPatchManifest {
            request_id,
            artifact_paths: artifact_paths.to_vec(),
            linked_image_path: linked_image_path.map(str::to_string),
            linked_image_size_bytes,
            linked_image_sha256: linked_image_sha256.map(str::to_string),
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

fn format_error_message(code: i32, detail_a: i32, detail_b: i32) -> String {
    let head = match code {
        41 => "missing function main(): i32",
        42 => "invalid function main signature; expected function main(): i32",
        43 => "multiple main declarations",
        1001 => "unexpected character while lexing",
        1002 => "unterminated string literal",
        1003 => "token overflow while lexing",
        2001 => "expected top-level function/extern/global declaration",
        2002 => "expected function after extern",
        2003 => "expected identifier",
        2004 => "expected '('",
        2005 => "expected ')'",
        2006 => "expected ':'",
        2007 => "expected ';'",
        2008 => "expected '{'",
        2009 => "expected '}'",
        2010 => "expected expression",
        2011 => "expected expression after return",
        2012 => "top-level parse guard exhausted",
        2013 => "parameter parse guard exhausted",
        2014 => "argument parse guard exhausted",
        2015 => "body parse guard exhausted",
        2016 => "call parse guard exhausted",
        2017 => "let declaration requires '='",
        2018 => "expected ']'",
        2019 => "expected 'in' in foreach",
        3001 => "incremental file overflow",
        3002 => "incremental file path was empty",
        4001 => "`from_*` conversion is mutating and cannot be used as an expression",
        _ => "incremental compiler error",
    };
    format!("{head} (code={code}, detail_a={detail_a}, detail_b={detail_b})")
}

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

fn clif_type_for_stasis_param_code(type_code: i32) -> &'static str {
    if type_code == 1 {
        "i32"
    } else {
        "i64"
    }
}

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
                let first_arg_type = clif_type_for_stasis_param_code(function_first_param_type_code);
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
                let first_arg_type = clif_type_for_stasis_param_code(function_first_param_type_code);
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
                let first_arg_type = clif_type_for_stasis_param_code(function_first_param_type_code);
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
                let second_arg_type = clif_type_for_stasis_param_code(function_first_param_type_code);
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

fn resolve_unique_i32_call_target_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    metrics: &[stasis_compiler::FunctionMetric],
) -> Option<String> {
    let target_id_hash = maybe_target_id_hash?;
    let mut matches = metrics.iter().filter(|candidate| {
        candidate.id_hash == target_id_hash
            && candidate.return_type == "i32"
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

fn resolve_unique_i32_single_arg_call_target_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    metrics: &[stasis_compiler::FunctionMetric],
    first_param_type_code: i32,
) -> Option<String> {
    let target_id_hash = maybe_target_id_hash?;
    let mut matches = metrics.iter().filter(|candidate| {
        candidate.id_hash == target_id_hash
            && candidate.return_type == "i32"
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

fn resolve_known_host_noarg_i32_extern_symbol_by_hash(target_id_hash: i32) -> Option<&'static str> {
    if target_id_hash == hash_identifier("host_cli_arg_count") {
        return Some("host_cli_arg_count");
    }
    if target_id_hash == hash_identifier("host_run_self_host_aot_cli_from_env") {
        return Some("host_run_self_host_aot_cli_from_env");
    }
    None
}

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
    use crate::self_host_runtime_bridge::{
        publish_cli_args_to_env, publish_source_files_to_env, publish_staged_bridge_paths_to_env,
        restore_cli_args_env, restore_source_files_env, restore_staged_bridge_paths_env,
        stasis_process_env_lock,
    };
    use object::Object;
    #[cfg(windows)]
    use stasis_dynload::{invoke_noarg_u64, Library as DynamicLibrary};
    use stasis_runner::swap::contracts::{CompileRequest, CompileStatus, RequestId, TargetMode};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static SIGN_ENV_LOCK: Mutex<()> = Mutex::new(());
    static STUB_FALLBACK_ENV_LOCK: Mutex<()> = Mutex::new(());
    static SUMMARY_ENV_LOCK: Mutex<()> = Mutex::new(());
    static ENTRY_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn identifier_hash_matches_incremental_function() {
        assert_eq!(hash_identifier("on_code_swap"), -663_287_521);
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
        assert!(clif.contains(
            "external host_write_aot_cli_summary(i64, i64, i64) -> i32"
        ));
        assert!(clif.contains("function %forward3(i64, i64, i64) -> i32"));
        assert!(clif.contains(
            "v3 = call %host_write_aot_cli_summary(v0, v1, v2)"
        ));
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
        assert!(clif.contains(
            "external host_load_source_file(i64, i32, i64, i64) -> i32"
        ));
        assert!(clif.contains("function %forward4(i64, i32, i64, i64) -> i32"));
        assert!(clif.contains(
            "v4 = call %host_load_source_file(v0, v1, v2, v3)"
        ));
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
        assert!(clif.contains(
            "external host_load_source_file(i64, i32, i64, i64) -> i32"
        ));
        assert!(clif.contains("function %forward4_add(i64, i32, i64, i64) -> i32"));
        assert!(clif.contains(
            "v4 = call %host_load_source_file(v0, v1, v2, v3)"
        ));
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
        let caller = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 0,
            id_hash: hash_identifier("main"),
            sig_hash: 11,
            body_hash: 12,
            return_type: "i32".to_string(),
            param_count: 0,
            first_param_type_code: 0,
            simple_i32_return_expr: None,
            simple_i32_return_call_target_id_hash: Some(hash_identifier("callee")),
            simple_i32_return_call_add_delta: None,
            simple_i32_return_call_one_arg_target_id_hash: None,
            simple_i32_return_call_one_arg_i32_literal: None,
            simple_i32_return_call_one_arg_arg_call_target_id_hash: None,
            simple_i32_return_two_call_left_target_id_hash: None,
            simple_i32_return_two_call_right_target_id_hash: None,
            simple_i32_return_two_call_op_code: None,
            simple_void_print_i32_literal: None,
            simple_void_print_i32_call_target_id_hash: None,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash: None,
            simple_void_print_i32_call_add_delta: None,
            clif_text: String::new(),
        };
        let callee = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 1,
            id_hash: hash_identifier("callee"),
            sig_hash: 21,
            body_hash: 22,
            return_type: "i32".to_string(),
            param_count: 0,
            first_param_type_code: 0,
            simple_i32_return_expr: None,
            simple_i32_return_call_target_id_hash: None,
            simple_i32_return_call_add_delta: None,
            simple_i32_return_call_one_arg_target_id_hash: None,
            simple_i32_return_call_one_arg_i32_literal: None,
            simple_i32_return_call_one_arg_arg_call_target_id_hash: None,
            simple_i32_return_two_call_left_target_id_hash: None,
            simple_i32_return_two_call_right_target_id_hash: None,
            simple_i32_return_two_call_op_code: None,
            simple_void_print_i32_literal: None,
            simple_void_print_i32_call_target_id_hash: None,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash: None,
            simple_void_print_i32_call_add_delta: None,
            clif_text: String::new(),
        };
        let metrics = vec![caller.clone(), callee.clone()];
        let resolved = resolve_unique_i32_call_target_symbol_by_hash(
            caller.simple_i32_return_call_target_id_hash,
            &metrics,
        )
        .expect("resolved");
        assert_eq!(resolved, aot_symbol_name(&callee));
    }

    #[test]
    fn resolve_simple_i32_return_call_target_symbol_rejects_one_arg_candidate_for_noarg_call() {
        let caller = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 0,
            id_hash: hash_identifier("main"),
            sig_hash: 11,
            body_hash: 12,
            return_type: "i32".to_string(),
            param_count: 0,
            first_param_type_code: 0,
            simple_i32_return_expr: None,
            simple_i32_return_call_target_id_hash: Some(hash_identifier("callee")),
            simple_i32_return_call_add_delta: None,
            simple_i32_return_call_one_arg_target_id_hash: None,
            simple_i32_return_call_one_arg_i32_literal: None,
            simple_i32_return_call_one_arg_arg_call_target_id_hash: None,
            simple_i32_return_two_call_left_target_id_hash: None,
            simple_i32_return_two_call_right_target_id_hash: None,
            simple_i32_return_two_call_op_code: None,
            simple_void_print_i32_literal: None,
            simple_void_print_i32_call_target_id_hash: None,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash: None,
            simple_void_print_i32_call_add_delta: None,
            clif_text: String::new(),
        };
        let callee = stasis_compiler::FunctionMetric {
            file_index: 0,
            ordinal: 1,
            id_hash: hash_identifier("callee"),
            sig_hash: 21,
            body_hash: 22,
            return_type: "i32".to_string(),
            param_count: 1,
            first_param_type_code: 1,
            simple_i32_return_expr: None,
            simple_i32_return_call_target_id_hash: None,
            simple_i32_return_call_add_delta: None,
            simple_i32_return_call_one_arg_target_id_hash: None,
            simple_i32_return_call_one_arg_i32_literal: None,
            simple_i32_return_call_one_arg_arg_call_target_id_hash: None,
            simple_i32_return_two_call_left_target_id_hash: None,
            simple_i32_return_two_call_right_target_id_hash: None,
            simple_i32_return_two_call_op_code: None,
            simple_void_print_i32_literal: None,
            simple_void_print_i32_call_target_id_hash: None,
            simple_void_print_i32_call_one_arg_arg_call_target_id_hash: None,
            simple_void_print_i32_call_add_delta: None,
            clif_text: String::new(),
        };
        let metrics = vec![caller.clone(), callee];
        let resolved =
            resolve_unique_i32_call_target_symbol_by_hash(caller.simple_i32_return_call_target_id_hash, &metrics);
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
        let resolved = resolve_known_host_two_arg_literal_first_second_param_i32_extern_symbol_by_hash(
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
    fn aot_compile_reports_missing_helper_binary() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_missing_helper_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return 0; }\n").expect("write source");

        let missing_helper = temp_root.join("missing-helper.exe");
        let config = AotCompileConfig {
            helper_path: Some(missing_helper),
            ..AotCompileConfig::default()
        };
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, temp_root.join("aot_artifacts"));
        let result = backend.compile(CompileRequest::new(
            RequestId(1),
            vec![source],
            TargetMode::AotProd,
        ));

        assert_eq!(result.status, CompileStatus::Failed);
        assert!(!result.diagnostics.is_empty());
        assert!(
            result.diagnostics[0]
                .message
                .contains("missing Cranelift AOT helper"),
            "unexpected diagnostic: {}",
            result.diagnostics[0].message
        );
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
                diagnostic
                    .message
                    .contains("unresolved direct call target for emitted function")
            }),
            "expected unresolved direct call target diagnostic"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_rejects_direct_call_target_with_signature_mismatch() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_signature_mismatch_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(); }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(137),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("unresolved direct call target for emitted function")
            }),
            "expected unresolved direct call target diagnostic on signature mismatch"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_noarg_extern_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_known_host_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_count(): i32;\nfunction main(): i32 { return host_cli_arg_count() + 10; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

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
        assert!(
            manifest.fallback_stub_symbols.is_empty(),
            "known host extern direct-call lowering should not fall back to stubs"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_one_arg_passthrough_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_known_host_one_arg_passthrough_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_set_summary_file(summary_file: ascii[]): i32;\nfunction forward(summary_file: ascii[]): i32 { return host_set_summary_file(summary_file); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(139),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host one-arg passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_two_arg_passthrough_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .join(format!("stasis_aot_known_host_two_arg_passthrough_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(index: i32, out_value: ascii[]): i32 { return host_cli_arg_value(index, out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(140),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host two-arg passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_three_arg_passthrough_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .join(format!("stasis_aot_known_host_three_arg_passthrough_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_write_aot_cli_summary(output_exe: ascii[], ir_bundle_path: ascii[], object_bundle_path: ascii[]): i32;\nfunction forward(output_exe: ascii[], ir_bundle_path: ascii[], object_bundle_path: ascii[]): i32 { return host_write_aot_cli_summary(output_exe, ir_bundle_path, object_bundle_path); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(141),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host three-arg passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_four_arg_passthrough_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .join(format!("stasis_aot_known_host_four_arg_passthrough_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_load_source_file(project_dir: ascii[], file_index: i32, out_path: ascii[], out_source: ascii[]): i32;\nfunction forward(project_dir: ascii[], file_index: i32, out_path: ascii[], out_source: ascii[]): i32 { return host_load_source_file(project_dir, file_index, out_path, out_source); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(142),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host four-arg passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_two_arg_literal_first_second_param_passthrough_direct_call_target(
    ) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .join(format!("stasis_aot_known_host_two_arg_lit_param_passthrough_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value(1, out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(143),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host two-arg literal+param passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_two_arg_literal_expression_first_second_param_passthrough_direct_call_target(
    ) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_aot_known_host_two_arg_lit_expr_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value(1 + 2, out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(150),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host two-arg literal-expression+param passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_two_arg_parenthesized_literal_expression_first_second_param_passthrough_direct_call_target(
    ) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_aot_known_host_two_arg_paren_lit_expr_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1 + 2), out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(151),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host two-arg parenthesized literal-expression+param passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_two_arg_parenthesized_literal_first_second_param_passthrough_direct_call_target(
    ) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_aot_known_host_two_arg_paren_lit_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1), out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(152),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host two-arg parenthesized literal+param passthrough direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_host_two_arg_parenthesized_literal_first_second_param_passthrough_add_delta_direct_call_target(
    ) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_aot_known_host_two_arg_paren_lit_param_passthrough_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1), out_value) - 4; }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(153),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("forward")),
            "known host two-arg parenthesized literal+param passthrough add-delta direct-call lowering should not fall back for forward()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_one_arg_literal_expression_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_one_arg_lit_expr_direct_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9 + 2); }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(146),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("main")),
            "one-arg literal-expression direct-call lowering should not fall back for main()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_one_arg_parenthesized_literal_expression_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .join(format!("stasis_aot_one_arg_paren_lit_expr_direct_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee((9 + 2)); }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(149),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest
                .fallback_stub_details
                .iter()
                .any(|detail| detail.id_hash == hash_identifier("main")),
            "one-arg parenthesized literal-expression direct-call lowering should not fall back for main()"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_accepts_known_runtime_entry_host_extern_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_known_runtime_entry_host_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "extern function host_run_self_host_aot_cli_from_env(): i32;\nfunction main(): i32 { return host_run_self_host_aot_cli_from_env(); }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());

        let result = backend.compile(CompileRequest::new(
            RequestId(138),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            manifest.fallback_stub_symbols.is_empty(),
            "known runtime entry host extern direct-call lowering should not fall back to stubs"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_rejects_unresolved_one_arg_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_unresolved_one_arg_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return missing(7); }\n").expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(134),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("unresolved one-arg direct call target for emitted function")
            }),
            "expected unresolved one-arg direct call target diagnostic"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_rejects_unresolved_one_arg_direct_call_arg_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_unresolved_one_arg_call_arg_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(missing()); }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(135),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("unresolved one-arg direct call argument target for emitted function")
            }),
            "expected unresolved one-arg direct call argument target diagnostic"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_rejects_unresolved_void_print_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_unresolved_void_print_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { print_i32(missing()); return; }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(132),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("unresolved void print_i32 call target for emitted function")
            }),
            "expected unresolved void print_i32 call target diagnostic"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_rejects_unresolved_two_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_unresolved_two_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function lhs(): i32 { return 1; }\nfunction main(): i32 { return lhs() + missing(); }\n",
        )
        .expect("write source");

        let mut backend = IncrementalCompilerBackend::new();
        let result = backend.compile(CompileRequest::new(
            RequestId(133),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Failed);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("unresolved two-call right target for emitted function")
            }),
            "expected unresolved two-call target diagnostic"
        );
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
        let helper = write_fake_aot_helper(&temp_root);

        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());
        let result = backend.compile(CompileRequest::new(
            RequestId(99),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        assert!(manifest_path.exists(), "manifest should be written");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert_eq!(manifest.request_id, 99);
        assert!(!manifest.artifact_paths.is_empty());
        assert!(manifest.linked_image_path.is_none());
        assert!(manifest.linked_image_sha256.is_none());
        for path in &manifest.artifact_paths {
            assert!(
                PathBuf::from(path).exists(),
                "artifact path should exist: {path}"
            );
        }
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_min_main_emits_cranelift_ir_and_no_fallback_stub() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_min_main_ir_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");
        let captured_clif = temp_root.join("captured_main.clif");
        let helper = write_recording_fake_aot_helper(&temp_root, &captured_clif);

        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());
        let result = backend.compile(CompileRequest::new(
            RequestId(109),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let clif = fs::read_to_string(&captured_clif).expect("read captured clif");
        assert!(
            clif.contains("function %fn_") && clif.contains("() -> i32"),
            "expected emitted i32 function signature in clif:\n{clif}"
        );
        assert!(
            clif.contains("iconst.i32 7"),
            "expected literal return in clif:\n{clif}"
        );
        assert!(
            clif.contains("return v0"),
            "expected value return in clif:\n{clif}"
        );

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        assert!(manifest_path.exists(), "manifest should be written");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            manifest.fallback_stub_symbols.is_empty(),
            "minimal main should not use fallback stubs"
        );
        assert!(
            manifest.fallback_stub_details.is_empty(),
            "minimal main should not report fallback details"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_compile_can_link_bundle_and_record_linked_image_in_manifest() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_link_manifest_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return 0; }\n").expect("write source");

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root.clone(),
            true,
        );
        let result = backend.compile(CompileRequest::new(
            RequestId(120),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert_eq!(manifest.request_id, 120);
        let linked = manifest
            .linked_image_path
            .as_ref()
            .expect("linked image path should be set");
        assert!(PathBuf::from(linked).exists(), "linked image should exist");
        assert!(manifest.linked_image_sha256.is_some());
        assert_eq!(result.aot_linked_image_path, Some(PathBuf::from(linked)));
        assert_eq!(
            result.aot_linked_image_sha256.as_deref(),
            manifest.linked_image_sha256.as_deref()
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_manifest_records_stub_fallback_details_with_body_hash_hints() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_fallback_manifest_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function main(): i32 { let value: i32 = 7; return value; }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());
        let result = backend.compile(CompileRequest::new(
            RequestId(131),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(!manifest.fallback_stub_symbols.is_empty());
        assert!(!manifest.fallback_stub_details.is_empty());
        assert_eq!(
            manifest.fallback_stub_symbols.len(),
            manifest.fallback_stub_details.len(),
            "fallback detail hints should track each fallback symbol"
        );
        for detail in &manifest.fallback_stub_details {
            assert!(
                manifest.fallback_stub_symbols.contains(&detail.symbol),
                "fallback detail symbol should be present in fallback symbols list"
            );
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_manifest_records_fallback_stub_for_unlowerable_entry_parse_function() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_entry_fallback_manifest_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let compiler_dir = temp_root.join("compiler");
        fs::create_dir_all(&compiler_dir).expect("create compiler dir");
        let source = compiler_dir.join("stasis_aot_cli_entry.stasis");
        fs::write(
            &source,
            "function compiler_cli_parse_from_argv(): i32 { let value: i32 = 0; return value; }\nfunction main(): i32 { return compiler_cli_parse_from_argv(); }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());
        let result = backend.compile(CompileRequest::new(
            RequestId(132),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            !manifest.fallback_stub_symbols.is_empty(),
            "unlowerable entry parse function should now be tracked as fallback stub"
        );
        assert!(
            !manifest.fallback_stub_details.is_empty(),
            "fallback detail hints should be present for unlowerable entry parse function"
        );
        assert_eq!(
            manifest.fallback_stub_symbols.len(),
            manifest.fallback_stub_details.len(),
            "fallback details should align with fallback symbols"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_manifest_has_no_fallback_for_lowerable_entry_function() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_lowerable_entry_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let compiler_dir = temp_root.join("compiler");
        fs::create_dir_all(&compiler_dir).expect("create compiler dir");
        let source = compiler_dir.join("stasis_aot_cli_entry.stasis");
        fs::write(
            &source,
            "extern function host_run_self_host_aot_cli_from_env(): i32;\nfunction compiler_cli_parse_from_argv(): i32 { return host_run_self_host_aot_cli_from_env(); }\nfunction main(): i32 { return compiler_cli_parse_from_argv(); }\n",
        )
        .expect("write source");
        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, artifact_root.clone());
        let result = backend.compile(CompileRequest::new(
            RequestId(139),
            vec![source],
            TargetMode::AotProd,
        ));
        assert_eq!(result.status, CompileStatus::Success);

        let manifest_path = artifact_root.join("last_patch_manifest.json");
        let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
        let manifest: AotPatchManifest =
            serde_json::from_str(&manifest_text).expect("parse manifest json");
        assert!(
            manifest.fallback_stub_symbols.is_empty(),
            "lowerable entry path should not require fallback stubs"
        );
        assert!(
            manifest.fallback_stub_details.is_empty(),
            "lowerable entry path should not emit fallback detail hints"
        );

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

        let helper = write_fake_aot_helper(&temp_root);
        let config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let mut backend =
            IncrementalCompilerBackend::with_aot_config(config, temp_root.join("aot_artifacts"));
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
        assert_eq!(
            symbols.len(),
            patch_set.functions.len(),
            "symbol mapping should cover all patched functions"
        );
        assert!(symbols.iter().all(|entry| entry.symbol.starts_with("fn_")));

        let hook_fn_id = result
            .hook_fn_id
            .expect("hook function id should be populated for on_code_swap");
        assert!(
            symbols.iter().any(|entry| entry.fn_id == hook_fn_id),
            "hook_fn_id should be present in emitted symbol mapping"
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_compile_with_real_linker_exports_emitted_symbols_when_available() {
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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
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

        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
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
            assert!(
                exports.iter().any(|name| name == &expected.symbol),
                "missing exported symbol {}",
                expected.symbol
            );
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_emitted_symbol_return_changes_when_body_changes_if_real_link_available() {
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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_body_change_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return 11; }\n").expect("write source");

        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            true,
        );

        let first = backend.compile(CompileRequest::new(
            RequestId(123),
            vec![source.clone()],
            TargetMode::AotProd,
        ));
        if first.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }
        let first_symbol = first
            .aot_function_symbols
            .as_ref()
            .and_then(|list| list.first())
            .map(|entry| entry.symbol.clone());
        let first_path = first.aot_linked_image_path.clone();
        let (Some(first_symbol), Some(first_path)) = (first_symbol, first_path) else {
            fs::remove_dir_all(&temp_root).ok();
            return;
        };
        let first_lib = DynamicLibrary::load(&first_path).expect("load first linked image");
        let first_ptr = first_lib
            .symbol_address(&first_symbol)
            .expect("resolve first emitted symbol");
        let first_value = invoke_noarg_u64(first_ptr).expect("invoke first emitted symbol");

        fs::write(&source, "function main(): i32 { return 12; }\n").expect("update source");
        let second = backend.compile(CompileRequest::new(
            RequestId(124),
            vec![source.clone()],
            TargetMode::AotProd,
        ));
        if second.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }
        let second_symbol = second
            .aot_function_symbols
            .as_ref()
            .and_then(|list| list.first())
            .map(|entry| entry.symbol.clone());
        let second_path = second.aot_linked_image_path.clone();
        let (Some(second_symbol), Some(second_path)) = (second_symbol, second_path) else {
            fs::remove_dir_all(&temp_root).ok();
            return;
        };
        let second_lib = DynamicLibrary::load(&second_path).expect("load second linked image");
        let second_ptr = second_lib
            .symbol_address(&second_symbol)
            .expect("resolve second emitted symbol");
        let second_value = invoke_noarg_u64(second_ptr).expect("invoke second emitted symbol");

        assert_ne!(
            first_value, second_value,
            "emitted symbol return value should change when body hash changes"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_emitted_symbol_executes_if_else_select_semantics_if_real_link_available() {
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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_if_else_semantics_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(
            &source,
            "function main(): i32 { let x: i32 = 2; if (x > 1) { return 77; } else { return 33; } }\n",
        )
        .expect("write source");

        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            true,
        );

        let first = backend.compile(CompileRequest::new(
            RequestId(125),
            vec![source.clone()],
            TargetMode::AotProd,
        ));
        if first.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }
        let first_symbol = first
            .aot_function_symbols
            .as_ref()
            .and_then(|list| list.first())
            .map(|entry| entry.symbol.clone());
        let first_path = first.aot_linked_image_path.clone();
        let (Some(first_symbol), Some(first_path)) = (first_symbol, first_path) else {
            fs::remove_dir_all(&temp_root).ok();
            return;
        };
        let first_lib = DynamicLibrary::load(&first_path).expect("load first linked image");
        let first_ptr = first_lib
            .symbol_address(&first_symbol)
            .expect("resolve first emitted symbol");
        let first_value = invoke_noarg_u64(first_ptr).expect("invoke first emitted symbol");
        assert_eq!(first_value as i32, 77);

        fs::write(
            &source,
            "function main(): i32 { let x: i32 = 0; if (x > 1) { return 77; } else { return 33; } }\n",
        )
        .expect("update source");
        let second = backend.compile(CompileRequest::new(
            RequestId(126),
            vec![source.clone()],
            TargetMode::AotProd,
        ));
        if second.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }
        let second_symbol = second
            .aot_function_symbols
            .as_ref()
            .and_then(|list| list.first())
            .map(|entry| entry.symbol.clone());
        let second_path = second.aot_linked_image_path.clone();
        let (Some(second_symbol), Some(second_path)) = (second_symbol, second_path) else {
            fs::remove_dir_all(&temp_root).ok();
            return;
        };
        let second_lib = DynamicLibrary::load(&second_path).expect("load second linked image");
        let second_ptr = second_lib
            .symbol_address(&second_symbol)
            .expect("resolve second emitted symbol");
        let second_value = invoke_noarg_u64(second_ptr).expect("invoke second emitted symbol");
        assert_eq!(second_value as i32, 33);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_emitted_symbol_executes_direct_call_semantics_if_real_link_available() {
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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
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

        let mut host = IncrementalCompilerHost::new();
        let parsed = host
            .compile_changed_files(std::slice::from_ref(&source))
            .expect("host parse");
        let main_metric = parsed
            .functions
            .iter()
            .find(|metric| metric.id_hash == hash_identifier("main"))
            .expect("main metric");
        let callee_metric = parsed
            .functions
            .iter()
            .find(|metric| metric.id_hash == hash_identifier("callee"))
            .expect("callee metric");
        let expected_main_symbol = aot_symbol_name(main_metric);
        let expected_callee_symbol = aot_symbol_name(callee_metric);

        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
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

    fn write_fake_aot_helper(temp_root: &Path) -> PathBuf {
        if cfg!(windows) {
            let helper = temp_root.join("fake-aot.cmd");
            let script = r#"@echo off
setlocal EnableDelayedExpansion
set OUT=
:loop
if "%~1"=="" goto done
if "%~1"=="--output" (
  set OUT=%~2
  shift
)
shift
goto loop
:done
if "%OUT%"=="" exit /b 2
echo fake-object>"%OUT%"
exit /b 0
"#;
            fs::write(&helper, script).expect("write fake helper script");
            helper
        } else {
            let helper = temp_root.join("fake-aot.sh");
            let script = r#"#!/usr/bin/env sh
OUT=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    OUT="$2"
    shift
  fi
  shift
done
if [ -z "$OUT" ]; then
  exit 2
fi
echo "fake-object" > "$OUT"
"#;
            fs::write(&helper, script).expect("write fake helper script");
            let status = Command::new("chmod")
                .arg("+x")
                .arg(&helper)
                .status()
                .expect("chmod fake helper");
            assert!(status.success(), "chmod fake helper should succeed");
            helper
        }
    }

    fn write_recording_fake_aot_helper(temp_root: &Path, captured_clif: &Path) -> PathBuf {
        if cfg!(windows) {
            let helper = temp_root.join("fake-aot-record.cmd");
            let script = format!(
                r#"@echo off
setlocal EnableDelayedExpansion
set IN=
set OUT=
:loop
if "%~1"=="" goto done
if "%~1"=="--input" (
  set IN=%~2
  shift
)
if "%~1"=="--output" (
  set OUT=%~2
  shift
)
shift
goto loop
:done
if "%OUT%"=="" exit /b 2
if not "%IN%"=="" copy /Y "%IN%" "{}" >nul
echo fake-object>"%OUT%"
exit /b 0
"#,
                captured_clif.display()
            );
            fs::write(&helper, script).expect("write recording fake helper script");
            helper
        } else {
            let helper = temp_root.join("fake-aot-record.sh");
            let script = format!(
                r#"#!/usr/bin/env sh
IN=""
OUT=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--input" ]; then
    IN="$2"
    shift
  elif [ "$1" = "--output" ]; then
    OUT="$2"
    shift
  fi
  shift
done
if [ -z "$OUT" ]; then
  exit 2
fi
if [ -n "$IN" ]; then
  cp "$IN" "{}"
fi
echo "fake-object" > "$OUT"
"#,
                captured_clif.display()
            );
            fs::write(&helper, script).expect("write recording fake helper script");
            let status = Command::new("chmod")
                .arg("+x")
                .arg(&helper)
                .status()
                .expect("chmod recording fake helper");
            assert!(status.success(), "chmod recording fake helper should succeed");
            helper
        }
    }

    fn write_fake_linker(temp_root: &Path) -> PathBuf {
        if cfg!(windows) {
            let linker = temp_root.join("fake-link.cmd");
            let script = r#"@echo off
setlocal EnableDelayedExpansion
set OUT=
for %%A in (%*) do (
  set ARG=%%~A
  echo !ARG! | findstr /B /C:"/OUT:" >nul
  if !errorlevel! == 0 (
    set OUT=!ARG:~5!
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

    fn copy_self_host_compiler_subset(repo_root: &Path, subset_root: &Path) {
        fs::create_dir_all(subset_root.join("compiler")).expect("create subset compiler dir");
        fs::create_dir_all(subset_root.join("src").join("stdlib"))
            .expect("create subset stdlib dir");
        fs::copy(
            repo_root.join("compiler").join("stasis_aot_cli_entry.stasis"),
            subset_root.join("compiler").join("stasis_aot_cli_entry.stasis"),
        )
        .expect("copy stasis_aot_cli_entry");
        fs::copy(
            repo_root.join("compiler").join("stasis_aot_cli_core.stasis"),
            subset_root.join("compiler").join("stasis_aot_cli_core.stasis"),
        )
        .expect("copy stasis_aot_cli_core");
        fs::copy(
            repo_root.join("compiler").join("simple_pass_compiler.stasis"),
            subset_root.join("compiler").join("simple_pass_compiler.stasis"),
        )
        .expect("copy incremental_compiler");
        fs::copy(
            repo_root.join("compiler").join("compiler_state.stasis"),
            subset_root.join("compiler").join("compiler_state.stasis"),
        )
        .expect("copy compiler_state");
        fs::copy(
            repo_root.join("src").join("stdlib").join("stdlib.stasis"),
            subset_root.join("src").join("stdlib").join("stdlib.stasis"),
        )
        .expect("copy stdlib");
    }

    fn collect_runtime_bridge_source_payload(project_root: &Path) -> Vec<(String, String)> {
        collect_stasis_files_for_self_host_project(project_root)
            .expect("collect source files")
            .into_iter()
            .map(|path| {
                let source =
                    fs::read_to_string(&path).expect("read source for runtime bridge payload");
                let normalized_path = path.to_string_lossy().replace('\\', "/");
                (normalized_path, source)
            })
            .collect()
    }

    fn parse_declared_function_name(line: &str) -> Option<&str> {
        let trimmed = line.trim_start();
        let mut rest = trimmed.strip_prefix("function ")?;
        rest = rest.trim_start();
        if let Some(after_inline) = rest.strip_prefix("@inline") {
            rest = after_inline.trim_start();
        }
        let mut end = 0usize;
        for ch in rest.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        if end == 0 {
            return None;
        }
        Some(&rest[..end])
    }

    fn collect_function_name_candidates_by_unsigned_id_hash(
        project_root: &Path,
    ) -> BTreeMap<u32, Vec<String>> {
        let mut by_hash: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        let paths = collect_stasis_files_for_self_host_project(project_root)
            .expect("collect source files for name-hash candidates");
        for path in paths {
            let source = fs::read_to_string(&path).expect("read source for name-hash candidates");
            for line in source.lines() {
                let Some(function_name) = parse_declared_function_name(line) else {
                    continue;
                };
                let unsigned_id_hash = hash_identifier(function_name).unsigned_abs();
                by_hash.entry(unsigned_id_hash).or_default().push(format!(
                    "{}:{}",
                    path.display(),
                    function_name
                ));
            }
        }
        by_hash
    }

    fn parse_aot_symbol_unsigned_id_hash(symbol: &str) -> Option<u32> {
        let mut parts = symbol.split('_');
        let tag = parts.next()?;
        if tag != "fn" {
            return None;
        }
        let id_hash = parts.next()?.parse::<u32>().ok()?;
        let _sig_hash = parts.next()?;
        let _ordinal = parts.next()?;
        Some(id_hash)
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

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let summary =
            match run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe) {
                Ok(value) => value,
                Err(message)
                    if message.contains("Application Control policy has blocked this file") =>
                {
                    fs::remove_dir_all(&temp_root).ok();
                    return;
                }
                Err(message) => panic!("self-host aot cli should succeed: {message}"),
            };
        assert_eq!(summary.source_file_count, 1);
        assert!(!summary.entry_symbol.is_empty());
        assert_eq!(summary.linked_image_path, output_exe);
        assert!(summary.linked_image_path.exists());
        assert!(summary.ir_bundle_path.exists());
        assert!(summary.object_bundle_path.exists());
        assert!(!summary.object_file_names.is_empty());
        assert!(
            summary
                .object_file_names
                .iter()
                .any(|name| name == "self_host_runtime_bridge.obj"),
            "expected linked object list to include runtime bridge object"
        );

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

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let signer = write_fake_signer(&temp_root);
        let old_signer = std::env::var("STASIS_AOT_SIGN_TOOL").ok();
        std::env::set_var("STASIS_AOT_SIGN_TOOL", &signer);

        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let result = run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe);
        if let Some(value) = old_signer {
            std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
        } else {
            std::env::remove_var("STASIS_AOT_SIGN_TOOL");
        }

        match result {
            Ok(_) => {
                let signed_marker = output_exe.with_file_name(format!(
                    "{}.signed",
                    output_exe
                        .file_name()
                        .expect("output file name")
                        .to_string_lossy()
                ));
                assert!(
                    signed_marker.exists(),
                    "expected signer marker {}",
                    signed_marker.display()
                );
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("self-host signing run should succeed: {message}"),
        }

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

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let result = run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe);
        let summary = match result {
            Ok(value) => value,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("self-host summary sidecar run should succeed: {message}"),
        };
        let sidecar_path = default_aot_cli_summary_sidecar_path(&output_exe);
        assert!(
            sidecar_path.exists(),
            "expected sidecar {}",
            sidecar_path.display()
        );
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
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = SUMMARY_ENV_LOCK.lock().expect("lock summary env");
        let old_summary = std::env::var("STASIS_AOT_SUMMARY_FILE").ok();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_summary_cfg_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let configured_summary = temp_root.join("custom").join("summary.json");
        std::env::set_var("STASIS_AOT_SUMMARY_FILE", &configured_summary);
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let result = run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe);
        if let Some(value) = old_summary {
            std::env::set_var("STASIS_AOT_SUMMARY_FILE", value);
        } else {
            std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
        }
        let summary = match result {
            Ok(value) => value,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => {
                panic!("self-host summary configured-path run should succeed: {message}")
            }
        };
        assert!(
            configured_summary.exists(),
            "expected configured summary path"
        );
        let sidecar_text =
            fs::read_to_string(&configured_summary).expect("read configured summary");
        let sidecar_summary: SelfHostedAotCliSummary =
            serde_json::from_str(&sidecar_text).expect("parse configured summary");
        assert_eq!(sidecar_summary.source_file_count, summary.source_file_count);
        assert_eq!(sidecar_summary.entry_symbol, summary.entry_symbol);
        assert_eq!(sidecar_summary.object_file_names, summary.object_file_names);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_prefers_rustc_live_backend_when_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_runtime_bridge_mode_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root.clone(),
            false,
        );
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe);
        match result {
            Ok(_) => {
                let mode_path = artifact_root.join("self_host_runtime_bridge.mode");
                assert!(mode_path.exists(), "expected runtime bridge mode marker");
                let mode = fs::read_to_string(&mode_path).expect("read mode marker");
                assert_eq!(mode.trim(), "rustc");
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("self-host runtime bridge mode run should succeed: {message}"),
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_rustc_source_uses_env_backed_staged_host_functions() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_runtime_bridge_source_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let object_path = temp_root.join("self_host_runtime_bridge.obj");
        emit_self_host_runtime_bridge_object_windows_rustc(&object_path)
            .expect("emit runtime bridge object should succeed");
        let source_path = object_path.with_extension("rs");
        let source_text = fs::read_to_string(&source_path).expect("read runtime bridge source");
        assert!(
            source_text.contains("STASIS_SELF_HOST_IR_BUNDLE_PATH"),
            "expected IR bundle env contract constant"
        );
        assert!(
            source_text.contains("STASIS_SELF_HOST_OBJECT_BUNDLE_PATH"),
            "expected object bundle env contract constant"
        );
        assert!(
            source_text.contains("STASIS_SELF_HOST_LINK_TEMPLATE_EXE"),
            "expected link template env contract constant"
        );
        assert!(
            source_text.contains("STASIS_SELF_HOST_SUMMARY_TEMPLATE_FILE"),
            "expected summary template env contract constant"
        );
        assert!(
            !source_text.contains(
                "fn host_emit_ir_from_compiler_state(_project_dir: *const u8, _out_ir_bundle: *mut u8) -> i32 { 1 }"
            ),
            "host_emit_ir_from_compiler_state should not be a hardcoded stub"
        );
        assert!(
            !source_text.contains(
                "fn host_run_cranelift_aot(_ir_bundle: *const u8, _out_object_bundle: *mut u8) -> i32 { 1 }"
            ),
            "host_run_cranelift_aot should not be a hardcoded stub"
        );
        assert!(
            !source_text.contains(
                "fn host_link_executable_from_objects(_object_bundle: *const u8, _output_exe: *const u8) -> i32 { 1 }"
            ),
            "host_link_executable_from_objects should not be a hardcoded stub"
        );
        assert!(
            !source_text.contains(
                "fn host_write_aot_cli_summary(_output_exe: *const u8, _ir_bundle: *const u8, _object_bundle: *const u8) -> i32 { 1 }"
            ),
            "host_write_aot_cli_summary should not be a hardcoded stub"
        );
        assert!(
            source_text.contains("fn host_run_self_host_aot_cli_from_env() -> i32"),
            "expected runtime entry bridge host function in rustc runtime bridge source"
        );
        assert!(
            source_text.contains("read_env_ascii(&source_key, out_source, 262144)"),
            "expected runtime bridge source-load buffer to match expanded source budget"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_runtime_bridge_clif_source_uses_env_backed_host_functions() {
        let source_text = build_self_host_runtime_bridge_clif(aot_call_conv());
        assert!(
            source_text.contains("global k_arg_count_key"),
            "expected arg-count env contract global"
        );
        assert!(
            source_text.contains("global k_source_count_key"),
            "expected source-count env contract global"
        );
        assert!(
            source_text.contains("global k_arg_key_0"),
            "expected indexed arg env contract globals"
        );
        assert!(
            source_text.contains("global k_source_path_key_0"),
            "expected indexed source path env contract globals"
        );
        assert!(
            source_text.contains("global k_source_text_key_0"),
            "expected indexed source text env contract globals"
        );
        assert!(
            source_text.contains("global k_ir_bundle_key"),
            "expected IR bundle env contract global"
        );
        assert!(
            source_text.contains("global k_object_bundle_key"),
            "expected object bundle env contract global"
        );
        assert!(
            source_text.contains("global k_link_template_exe_key"),
            "expected link template env contract global"
        );
        assert!(
            source_text.contains("global k_summary_template_key"),
            "expected summary template env contract global"
        );
        assert!(
            source_text.contains("external GetEnvironmentVariableA"),
            "expected environment variable bridge binding"
        );
        assert!(
            source_text.contains("external CopyFileA"),
            "expected file copy bridge binding"
        );
        assert!(
            source_text.contains("function %read_env_count_i32(i64) -> i32"),
            "expected env-backed count reader helper"
        );
        assert!(
            source_text.contains("function %select_arg_key(i32) -> i64"),
            "expected arg key selector helper"
        );
        assert!(
            source_text.contains("function %select_source_path_key(i32) -> i64"),
            "expected source path key selector helper"
        );
        assert!(
            source_text.contains("function %select_source_text_key(i32) -> i64"),
            "expected source text key selector helper"
        );
        assert!(
            source_text.contains("function %host_cli_arg_count() -> i32"),
            "expected live cli arg-count bridge function"
        );
        assert!(
            source_text.contains("function %host_cli_arg_value(i32, i64) -> i32"),
            "expected live cli arg-value bridge function"
        );
        assert!(
            source_text.contains("function %host_source_file_count(i64) -> i32"),
            "expected live source-count bridge function"
        );
        assert!(
            source_text.contains("function %host_load_source_file(i64, i32, i64, i64) -> i32"),
            "expected live source-load bridge function"
        );
        assert!(
            source_text.contains("iconst.i32 262144"),
            "expected clif runtime bridge source-load buffer to match expanded source budget"
        );
        assert!(
            source_text.contains("function %host_emit_ir_from_compiler_state(i64, i64) -> i32"),
            "expected staged ir bridge function"
        );
        assert!(
            source_text.contains("function %host_run_cranelift_aot(i64, i64) -> i32"),
            "expected staged object bridge function"
        );
        assert!(
            source_text.contains("function %host_link_executable_from_objects(i64, i64) -> i32"),
            "expected staged link bridge function"
        );
        assert!(
            source_text.contains("function %host_write_aot_cli_summary(i64, i64, i64) -> i32"),
            "expected staged summary bridge function"
        );
        assert!(
            source_text.contains("function %host_run_self_host_aot_cli_from_env() -> i32"),
            "expected runtime entry bridge function"
        );
        assert!(
            source_text.contains("global_value k_ir_bundle_key"),
            "expected staged ir bridge to read env-backed key"
        );
        assert!(
            !source_text
                .contains("function %host_cli_arg_count() -> i32 {cc} {\nblock0:\nv0 = iconst.i32 0\nreturn v0\n}"),
            "host_cli_arg_count should not be hardcoded zero"
        );
        assert!(
            !source_text
                .contains("function %host_cli_arg_value(i32, i64) -> i32 {cc} {\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}"),
            "host_cli_arg_value should not be hardcoded failure"
        );
        assert!(
            !source_text
                .contains("function %host_source_file_count(i64) -> i32 {cc} {\nblock0:\nv0 = iconst.i32 0\nreturn v0\n}"),
            "host_source_file_count should not be hardcoded zero"
        );
        assert!(
            !source_text
                .contains("function %host_load_source_file(i64, i32, i64, i64) -> i32 {cc} {\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}"),
            "host_load_source_file should not be hardcoded failure"
        );
    }

    #[test]
    fn self_host_aot_cli_rejects_stub_fallback_by_default() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = STUB_FALLBACK_ENV_LOCK
            .lock()
            .expect("lock stub fallback env");
        let old_strict = std::env::var("STASIS_AOT_STRICT_SELF_HOST").ok();
        std::env::remove_var("STASIS_AOT_ALLOW_STUB_FALLBACK");
        std::env::set_var("STASIS_AOT_STRICT_SELF_HOST", "1");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_strict_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "function main(): i32 { let value: i32 = 7; return value; }\n",
        )
        .expect("write source");

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let result = run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe);
        if let Some(value) = old_strict {
            std::env::set_var("STASIS_AOT_STRICT_SELF_HOST", value);
        } else {
            std::env::remove_var("STASIS_AOT_STRICT_SELF_HOST");
        }
        match result {
            Ok(_) => panic!("expected strict mode to reject stub fallback"),
            Err(message) => {
                assert!(
                    message.contains("strict mode rejected stub fallback lowering"),
                    "unexpected error: {message}"
                );
            }
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_can_allow_stub_fallback_temporarily() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let _guard = STUB_FALLBACK_ENV_LOCK
            .lock()
            .expect("lock stub fallback env");
        let old = std::env::var("STASIS_AOT_ALLOW_STUB_FALLBACK").ok();
        let old_strict = std::env::var("STASIS_AOT_STRICT_SELF_HOST").ok();
        std::env::set_var("STASIS_AOT_STRICT_SELF_HOST", "1");
        std::env::set_var("STASIS_AOT_ALLOW_STUB_FALLBACK", "1");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_allow_stub_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "function main(): i32 { let value: i32 = 7; return value; }\n",
        )
        .expect("write source");

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let result = run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe);
        if let Some(value) = old {
            std::env::set_var("STASIS_AOT_ALLOW_STUB_FALLBACK", value);
        } else {
            std::env::remove_var("STASIS_AOT_ALLOW_STUB_FALLBACK");
        }
        if let Some(value) = old_strict {
            std::env::set_var("STASIS_AOT_STRICT_SELF_HOST", value);
        } else {
            std::env::remove_var("STASIS_AOT_STRICT_SELF_HOST");
        }
        match result {
            Ok(summary) => {
                assert_eq!(summary.source_file_count, 1);
                assert!(summary.linked_image_path.exists());
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("allow-stub-fallback run should succeed: {message}"),
        }

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

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);
        let compile_config = AotCompileConfig {
            helper_path: Some(helper),
            ..AotCompileConfig::default()
        };
        let link_config = AotLinkConfig {
            linker_path: Some(linker),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend_first = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root.clone(),
            false,
        );
        let output_exe = if cfg!(windows) {
            temp_root.join("program.exe")
        } else {
            temp_root.join("program.out")
        };

        let first =
            match run_self_host_aot_cli_with_backend(&mut backend_first, &project_dir, &output_exe)
            {
                Ok(value) => value,
                Err(message)
                    if message.contains("Application Control policy has blocked this file") =>
                {
                    fs::remove_dir_all(&temp_root).ok();
                    if let Some(value) = &old_signer {
                        std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
                    } else {
                        std::env::remove_var("STASIS_AOT_SIGN_TOOL");
                    }
                    return;
                }
                Err(message) => panic!("first run should succeed: {message}"),
            };
        let mut backend_second = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            AotCompileConfig {
                helper_path: backend_first.aot_compile_config.helper_path.clone(),
                ..AotCompileConfig::default()
            },
            AotLinkConfig {
                linker_path: backend_first.aot_link_config.linker_path.clone(),
            },
            artifact_root.clone(),
            false,
        );
        let second =
            run_self_host_aot_cli_with_backend(&mut backend_second, &project_dir, &output_exe)
                .expect("second run should succeed");

        assert_eq!(first.source_file_count, second.source_file_count);
        assert_eq!(first.entry_symbol, second.entry_symbol);
        assert_eq!(first.linked_image_path, second.linked_image_path);
        assert_eq!(first.object_file_names, second.object_file_names);

        let ir_bundle_path = artifact_root.join("self_host_ir_bundle.json");
        let object_bundle_path = artifact_root.join("self_host_object_bundle.json");
        assert!(
            ir_bundle_path.exists(),
            "expected staged ir bundle metadata"
        );
        assert!(
            object_bundle_path.exists(),
            "expected staged object bundle metadata"
        );

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = &old_signer {
            std::env::set_var("STASIS_AOT_SIGN_TOOL", value);
        } else {
            std::env::remove_var("STASIS_AOT_SIGN_TOOL");
        }
    }

    #[test]
    fn self_host_aot_cli_stage1_stage2_metadata_smoke_matches_core_contract() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_stage12_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "function helper(): i32 { return 2; }\nfunction main(): i32 { return helper() + 5; }\n",
        )
        .expect("write source");

        let helper = write_fake_aot_helper(&temp_root);
        let linker = write_fake_linker(&temp_root);

        let artifact_root_stage1 = temp_root.join("aot_stage1");
        let mut backend_stage1 = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            AotCompileConfig {
                helper_path: Some(helper.clone()),
                ..AotCompileConfig::default()
            },
            AotLinkConfig {
                linker_path: Some(linker.clone()),
            },
            artifact_root_stage1.clone(),
            false,
        );
        let output_stage1 = if cfg!(windows) {
            temp_root.join("stage1.exe")
        } else {
            temp_root.join("stage1.out")
        };
        let summary_stage1 = match run_self_host_aot_cli_with_backend(
            &mut backend_stage1,
            &project_dir,
            &output_stage1,
        ) {
            Ok(value) => value,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("stage1 build should succeed: {message}"),
        };

        let ir_stage1: SelfHostIrBundle = serde_json::from_str(
            &fs::read_to_string(artifact_root_stage1.join("self_host_ir_bundle.json"))
                .expect("read stage1 ir bundle"),
        )
        .expect("parse stage1 ir bundle");
        let obj_stage1: SelfHostObjectBundle = serde_json::from_str(
            &fs::read_to_string(artifact_root_stage1.join("self_host_object_bundle.json"))
                .expect("read stage1 object bundle"),
        )
        .expect("parse stage1 object bundle");

        let artifact_root_stage2 = temp_root.join("aot_stage2");
        let mut backend_stage2 = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            AotCompileConfig {
                helper_path: Some(helper),
                ..AotCompileConfig::default()
            },
            AotLinkConfig {
                linker_path: Some(linker),
            },
            artifact_root_stage2.clone(),
            false,
        );
        let output_stage2 = if cfg!(windows) {
            temp_root.join("stage2.exe")
        } else {
            temp_root.join("stage2.out")
        };
        let summary_stage2 =
            run_self_host_aot_cli_with_backend(&mut backend_stage2, &project_dir, &output_stage2)
                .expect("stage2 build should succeed");

        let ir_stage2: SelfHostIrBundle = serde_json::from_str(
            &fs::read_to_string(artifact_root_stage2.join("self_host_ir_bundle.json"))
                .expect("read stage2 ir bundle"),
        )
        .expect("parse stage2 ir bundle");
        let obj_stage2: SelfHostObjectBundle = serde_json::from_str(
            &fs::read_to_string(artifact_root_stage2.join("self_host_object_bundle.json"))
                .expect("read stage2 object bundle"),
        )
        .expect("parse stage2 object bundle");

        let names_stage1: Vec<String> = ir_stage1
            .object_paths
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .expect("object file name")
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        let names_stage2: Vec<String> = ir_stage2
            .object_paths
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .expect("object file name")
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        assert_eq!(
            summary_stage1.source_file_count,
            summary_stage2.source_file_count
        );
        assert_eq!(summary_stage1.entry_symbol, summary_stage2.entry_symbol);
        assert_eq!(
            summary_stage1.object_file_names,
            summary_stage2.object_file_names
        );
        assert_eq!(ir_stage1.source_file_count, ir_stage2.source_file_count);
        assert_eq!(ir_stage1.entry_symbol, ir_stage2.entry_symbol);
        assert_eq!(obj_stage1.entry_symbol, obj_stage2.entry_symbol);
        assert_eq!(names_stage1, names_stage2);
        assert_eq!(obj_stage1.object_paths.len(), obj_stage2.object_paths.len());

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_emits_runnable_executable_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_real_exe_smoke = std::env::var("STASIS_RUN_REAL_AOT_EXE_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_real_exe_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_real_exe_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        match result {
            Ok(summary) => {
                assert_eq!(summary.source_file_count, 1);
                assert!(summary.linked_image_path.exists());
                let status = Command::new(&summary.linked_image_path)
                    .status()
                    .expect("run compiled executable");
                assert_eq!(
                    status.code(),
                    Some(7),
                    "compiled executable should return exit code 7"
                );
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(message) => {
                panic!("real toolchain self-host aot-cli run should succeed: {message}")
            }
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_runs_conditional_addition_main_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_real_exe_smoke = std::env::var("STASIS_RUN_REAL_AOT_EXE_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_real_exe_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_real_exe_if_add_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "function add_pair(left: i32, right: i32): i32 { return left + right; }\nfunction main(): i32 { let total: i32 = add_pair(2, 3); if (total > 4) { return total + 2; } return 0; }\n",
        )
        .expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        match result {
            Ok(summary) => {
                assert_eq!(summary.source_file_count, 1);
                assert!(summary.linked_image_path.exists());
                let status = Command::new(&summary.linked_image_path)
                    .status()
                    .expect("run compiled executable");
                assert_eq!(
                    status.code(),
                    Some(7),
                    "compiled executable should return exit code 7 for conditional addition main"
                );
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(message) => {
                panic!("real toolchain self-host aot-cli run should succeed: {message}")
            }
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_runs_for_accumulation_main_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_real_exe_smoke = std::env::var("STASIS_RUN_REAL_AOT_EXE_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_real_exe_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_real_exe_for_sum_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "function main(): i32 { let sum: i32 = 0; for (let i: i32 = 0; i < 4; i += 1) { sum += i; } return sum; }\n",
        )
        .expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        match result {
            Ok(summary) => {
                assert_eq!(summary.source_file_count, 1);
                assert!(summary.linked_image_path.exists());
                let status = Command::new(&summary.linked_image_path)
                    .status()
                    .expect("run compiled executable");
                assert_eq!(
                    status.code(),
                    Some(6),
                    "compiled executable should return exit code 6 for for accumulation main"
                );
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(message) => {
                panic!("real toolchain self-host aot-cli run should succeed: {message}")
            }
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_runs_struct_global_main_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_real_exe_smoke = std::env::var("STASIS_RUN_REAL_AOT_EXE_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_real_exe_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_real_exe_struct_global_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "struct Enemy { hp: i32; }\nglobal State { score: i32; first_enemy: Enemy; }\nfunction main(): i32 { return 7; }\n",
        )
        .expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        match result {
            Ok(summary) => {
                assert_eq!(summary.source_file_count, 1);
                assert!(summary.linked_image_path.exists());
                let status = Command::new(&summary.linked_image_path)
                    .status()
                    .expect("run compiled executable");
                assert_eq!(
                    status.code(),
                    Some(7),
                    "compiled executable should return exit code 7 for struct/global main"
                );
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(message) => {
                panic!("real toolchain self-host aot-cli run should succeed: {message}")
            }
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_compiler_subset_builds_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_SELF_HOST_COMPILER_SUBSET_BUILD_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

        let _guard = ENTRY_ENV_LOCK.lock().expect("lock entry env");

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        let old_entry = std::env::var("STASIS_AOT_ENTRY_FILE").ok();

        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);
        std::env::set_var("STASIS_AOT_ENTRY_FILE", "compiler/stasis_aot_cli_entry.stasis");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_subset_build_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let subset_root = temp_root.join("subset");

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        copy_self_host_compiler_subset(&repo_root, &subset_root);

        let output_exe = temp_root.join("stage1_compiler.exe");
        let result = run_self_host_aot_cli(&subset_root, &output_exe);
        match result {
            Ok(summary) => {
                assert_eq!(
                    summary.source_file_count, 5,
                    "expected subset closure to include exactly 5 files"
                );
                assert!(summary.linked_image_path.exists());
                assert_eq!(summary.linked_image_path, output_exe);
            }
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                if let Some(value) = old_entry {
                    std::env::set_var("STASIS_AOT_ENTRY_FILE", value);
                } else {
                    std::env::remove_var("STASIS_AOT_ENTRY_FILE");
                }
                return;
            }
            Err(message) => panic!("self-host compiler subset build should succeed: {message}"),
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
        if let Some(value) = old_entry {
            std::env::set_var("STASIS_AOT_ENTRY_FILE", value);
        } else {
            std::env::remove_var("STASIS_AOT_ENTRY_FILE");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_stage1_executable_drives_stage2_summary_parity_if_real_toolchain_available(
    ) {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_SELF_HOST_STAGE1_EXEC_PARITY_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

        let _guard = ENTRY_ENV_LOCK.lock().expect("lock entry env");

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        let old_entry = std::env::var("STASIS_AOT_ENTRY_FILE").ok();
        let old_summary = std::env::var("STASIS_AOT_SUMMARY_FILE").ok();

        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);
        std::env::set_var("STASIS_AOT_ENTRY_FILE", "compiler/stasis_aot_cli_entry.stasis");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_stage1_exec_parity_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let subset_root = temp_root.join("subset");
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        copy_self_host_compiler_subset(&repo_root, &subset_root);

        let stage1_output = temp_root.join("stage1_compiler.exe");
        let stage1_summary_path = temp_root.join("stage1_summary.json");
        std::env::set_var("STASIS_AOT_SUMMARY_FILE", &stage1_summary_path);
        let stage1_result = run_self_host_aot_cli(&subset_root, &stage1_output);
        if let Some(value) = &old_summary {
            std::env::set_var("STASIS_AOT_SUMMARY_FILE", value);
        } else {
            std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
        }
        let stage1_summary = match stage1_result {
            Ok(summary) => summary,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                if let Some(value) = old_entry {
                    std::env::set_var("STASIS_AOT_ENTRY_FILE", value);
                } else {
                    std::env::remove_var("STASIS_AOT_ENTRY_FILE");
                }
                if let Some(value) = old_summary {
                    std::env::set_var("STASIS_AOT_SUMMARY_FILE", value);
                } else {
                    std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
                }
                return;
            }
            Err(message) => panic!("stage1 compiler subset build should succeed: {message}"),
        };

        assert!(stage1_summary.linked_image_path.exists());
        assert!(
            stage1_summary.ir_bundle_path.exists(),
            "stage1 ir bundle path should exist"
        );
        assert!(
            stage1_summary.object_bundle_path.exists(),
            "stage1 object bundle path should exist"
        );
        assert!(
            stage1_summary_path.exists(),
            "stage1 summary sidecar path should exist"
        );

        let stage1_summary_sidecar: SelfHostedAotCliSummary = serde_json::from_str(
            &fs::read_to_string(&stage1_summary_path).expect("read stage1 summary sidecar"),
        )
        .expect("parse stage1 summary sidecar");
        let stage1_manifest_path = stage1_summary
            .ir_bundle_path
            .with_file_name("last_patch_manifest.json");
        let stage1_fallback_stub_symbols: Vec<String> = fs::read_to_string(&stage1_manifest_path)
            .ok()
            .and_then(|text| serde_json::from_str::<AotPatchManifest>(&text).ok())
            .map(|manifest| manifest.fallback_stub_symbols)
            .unwrap_or_default();
        let stage1_fallback_stub_details: Vec<AotFallbackStubDetail> =
            fs::read_to_string(&stage1_manifest_path)
                .ok()
                .and_then(|text| serde_json::from_str::<AotPatchManifest>(&text).ok())
                .map(|manifest| manifest.fallback_stub_details)
                .unwrap_or_default();
        let stage1_fallback_preview = stage1_fallback_stub_symbols
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let stage1_fallback_body_hash_preview = stage1_fallback_stub_details
            .iter()
            .take(6)
            .map(|detail| format!("{}:{}", detail.symbol, detail.body_hash))
            .collect::<Vec<_>>()
            .join(", ");
        let function_name_candidates_by_hash =
            collect_function_name_candidates_by_unsigned_id_hash(&subset_root);

        let stage2_output_exe = temp_root.join("stage2_compiler.exe");
        let stage2_summary_path = temp_root.join("stage2_summary.json");
        let stage2_args = vec![
            "--project-dir".to_string(),
            subset_root.display().to_string(),
            "--out".to_string(),
            stage2_output_exe.display().to_string(),
            "--summary-file".to_string(),
            stage2_summary_path.display().to_string(),
        ];
        let source_payload = collect_runtime_bridge_source_payload(&subset_root);
        let cli_snapshot = publish_cli_args_to_env(&stage2_args, Some(stage2_summary_path.as_path()));
        let source_snapshot = publish_source_files_to_env(&source_payload);
        let staged_snapshot = publish_staged_bridge_paths_to_env(
            &stage1_summary.ir_bundle_path,
            &stage1_summary.object_bundle_path,
            &stage1_summary.linked_image_path,
            Some(&stage1_summary_path),
        );
        let stage2_run = Command::new(&stage1_summary.linked_image_path)
            .current_dir(&subset_root)
            .output();
        restore_staged_bridge_paths_env(staged_snapshot);
        restore_source_files_env(source_snapshot);
        restore_cli_args_env(cli_snapshot);

        match stage2_run {
            Ok(value) => {
                let exit_code = value.status.code();
                let exit_fallback_symbol_hint = exit_code.and_then(|code| {
                    stage1_fallback_stub_details
                        .iter()
                        .find(|detail| detail.body_hash == code)
                        .map(|detail| detail.symbol.clone())
                });
                let exit_fallback_name_hint = exit_fallback_symbol_hint.as_ref().and_then(|symbol| {
                    let unsigned_id_hash = parse_aot_symbol_unsigned_id_hash(symbol)?;
                    function_name_candidates_by_hash
                        .get(&unsigned_id_hash)
                        .map(|entries| entries.join(", "))
                });
                assert_eq!(
                    exit_code,
                    Some(0),
                    "compiled stage1 executable should complete stage2 invocation successfully (exit={:?})\nstdout:\n{}\nstderr:\n{}\nstage1_fallback_stub_count={}\nstage1_fallback_stub_preview={}\nstage1_fallback_stub_body_hash_preview={}\nexit_fallback_symbol_hint={}\nexit_fallback_name_hint={}",
                    exit_code,
                    String::from_utf8_lossy(&value.stdout),
                    String::from_utf8_lossy(&value.stderr),
                    stage1_fallback_stub_symbols.len(),
                    stage1_fallback_preview,
                    stage1_fallback_body_hash_preview,
                    exit_fallback_symbol_hint.unwrap_or_else(|| "none".to_string()),
                    exit_fallback_name_hint.unwrap_or_else(|| "none".to_string())
                );
            }
            Err(error) if error.to_string().contains("blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                if let Some(value) = old_entry {
                    std::env::set_var("STASIS_AOT_ENTRY_FILE", value);
                } else {
                    std::env::remove_var("STASIS_AOT_ENTRY_FILE");
                }
                if let Some(value) = old_summary {
                    std::env::set_var("STASIS_AOT_SUMMARY_FILE", value);
                } else {
                    std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
                }
                return;
            }
            Err(error) => panic!("failed to run stage1 compiler executable: {error}"),
        }

        assert!(
            stage2_output_exe.exists(),
            "stage2 output executable should be produced by stage1 run"
        );
        assert!(
            stage2_summary_path.exists(),
            "stage2 summary path should be produced by stage1 run"
        );

        let stage2_summary: SelfHostedAotCliSummary = serde_json::from_str(
            &fs::read_to_string(&stage2_summary_path).expect("read stage2 summary"),
        )
        .expect("parse stage2 summary");
        assert_eq!(
            stage1_summary_sidecar.source_file_count,
            stage2_summary.source_file_count
        );
        assert_eq!(stage1_summary_sidecar.entry_symbol, stage2_summary.entry_symbol);
        assert_eq!(
            stage1_summary_sidecar.object_file_names,
            stage2_summary.object_file_names
        );
        assert_eq!(
            fs::read(&stage2_output_exe).expect("read stage2 output executable"),
            fs::read(&stage1_summary.linked_image_path).expect("read stage1 output executable"),
            "stage2 executable should match staged runtime bridge link template executable"
        );

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
        if let Some(value) = old_entry {
            std::env::set_var("STASIS_AOT_ENTRY_FILE", value);
        } else {
            std::env::remove_var("STASIS_AOT_ENTRY_FILE");
        }
        if let Some(value) = old_summary {
            std::env::set_var("STASIS_AOT_SUMMARY_FILE", value);
        } else {
            std::env::remove_var("STASIS_AOT_SUMMARY_FILE");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_aot_cli_signed_executable_smoke_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_signed_smoke = std::env::var("STASIS_RUN_SIGNED_SELF_HOST_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_signed_smoke {
            return;
        }
        let Some(signer_path) = std::env::var("STASIS_AOT_SIGN_TOOL").ok() else {
            return;
        };

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);
        std::env::set_var("STASIS_AOT_SIGN_TOOL", &signer_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_signed_exe_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(&source, "function main(): i32 { return 7; }\n").expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        match result {
            Ok(summary) => {
                assert_eq!(summary.source_file_count, 1);
                assert!(summary.linked_image_path.exists());
                let status = Command::new(&summary.linked_image_path)
                    .status()
                    .expect("run compiled signed executable");
                assert_eq!(
                    status.code(),
                    Some(7),
                    "compiled signed executable should return exit code 7"
                );
            }
            Err(message) => panic!("signed self-host executable smoke should succeed: {message}"),
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_live_argc_smoke_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_RUNTIME_BRIDGE_ARGC_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_runtime_bridge_argc_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "extern function host_cli_arg_count(): i32;\nfunction main(): i32 { return host_cli_arg_count() + 10; }\n",
        )
        .expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        let summary = match result {
            Ok(value) => value,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(message) => panic!("runtime bridge live argc build should succeed: {message}"),
        };

        let status = Command::new(&summary.linked_image_path)
            .env("STASIS_SELF_HOST_ARG_COUNT", "5")
            .status();
        match status {
            Ok(value) => {
                assert_eq!(
                    value.code(),
                    Some(15),
                    "runtime bridge should surface STASIS_SELF_HOST_ARG_COUNT through host_cli_arg_count"
                );
            }
            Err(error) if error.to_string().contains("blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(error) => panic!("failed to run runtime bridge argc executable: {error}"),
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_live_source_count_smoke_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_RUNTIME_BRIDGE_SOURCE_COUNT_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let old_helper = std::env::var("STASIS_CRANELIFT_AOT").ok();
        let old_linker = std::env::var("STASIS_AOT_LINKER").ok();
        std::env::set_var("STASIS_CRANELIFT_AOT", &helper_path);
        std::env::set_var("STASIS_AOT_LINKER", &linker_path);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_runtime_bridge_sources_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let project_dir = temp_root.join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");
        let source = project_dir.join("main.stasis");
        fs::write(
            &source,
            "extern function host_source_file_count(project_dir: ascii[]): i32;\nfunction main(): i32 { return host_source_file_count(\"x\") + 20; }\n",
        )
        .expect("write source");
        let output_exe = temp_root.join("program.exe");

        let result = run_self_host_aot_cli(&project_dir, &output_exe);
        let summary = match result {
            Ok(value) => value,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(message) => {
                panic!("runtime bridge live source-count build should succeed: {message}")
            }
        };

        let status = Command::new(&summary.linked_image_path)
            .env("STASIS_SELF_HOST_SOURCE_FILE_COUNT", "4")
            .status();
        match status {
            Ok(value) => {
                assert_eq!(
                    value.code(),
                    Some(24),
                    "runtime bridge should surface STASIS_SELF_HOST_SOURCE_FILE_COUNT through host_source_file_count"
                );
            }
            Err(error) if error.to_string().contains("blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                if let Some(value) = old_helper {
                    std::env::set_var("STASIS_CRANELIFT_AOT", value);
                } else {
                    std::env::remove_var("STASIS_CRANELIFT_AOT");
                }
                if let Some(value) = old_linker {
                    std::env::set_var("STASIS_AOT_LINKER", value);
                } else {
                    std::env::remove_var("STASIS_AOT_LINKER");
                }
                return;
            }
            Err(error) => panic!("failed to run runtime bridge source-count executable: {error}"),
        }

        fs::remove_dir_all(&temp_root).ok();
        if let Some(value) = old_helper {
            std::env::set_var("STASIS_CRANELIFT_AOT", value);
        } else {
            std::env::remove_var("STASIS_CRANELIFT_AOT");
        }
        if let Some(value) = old_linker {
            std::env::set_var("STASIS_AOT_LINKER", value);
        } else {
            std::env::remove_var("STASIS_AOT_LINKER");
        }
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_live_staged_externs_smoke_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_RUNTIME_BRIDGE_STAGED_EXTERN_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_runtime_bridge_staged_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let template_ir = temp_root.join("i");
        let template_object = temp_root.join("j");
        let template_exe = temp_root.join("t");
        let template_summary = temp_root.join("u");
        fs::write(&template_ir, "{\"entry\":\"main\"}\n").expect("write template ir bundle");
        fs::write(&template_object, "{\"entry\":\"main\"}\n")
            .expect("write template object bundle");
        fs::write(&template_exe, "template-exe").expect("write template exe");
        fs::write(&template_summary, "{\"source_file_count\":1}\n")
            .expect("write template summary");

        let bridge_object = temp_root.join("self_host_runtime_bridge.obj");
        emit_self_host_runtime_bridge_object_windows_rustc(&bridge_object)
            .expect("emit runtime bridge object");

        let call_conv = aot_call_conv();
        let driver_clif = format!(
            "external host_set_summary_file(i64) -> i32 {cc}\n\
external host_emit_ir_from_compiler_state(i64, i64) -> i32 {cc}\n\
external host_run_cranelift_aot(i64, i64) -> i32 {cc}\n\
external host_link_executable_from_objects(i64, i64) -> i32 {cc}\n\
external host_write_aot_cli_summary(i64, i64, i64) -> i32 {cc}\n\
function %bridge_stage_main() -> i32 {cc} {{\n\
block0:\n\
v0 = iconst.i64 0\n\
v1 = iconst.i64 1\n\
v2 = iconst.i8 0\n\
v3 = iconst.i8 111\n\
v4 = iconst.i8 115\n\
v5 = iconst.i32 0\n\
v6 = iconst.i32 10\n\
v7 = iconst.i32 20\n\
v8 = iconst.i32 30\n\
v9 = iconst.i32 40\n\
v10 = stack_slot.i64\n\
v11 = iadd v10, v1\n\
store v3, v10\n\
store v2, v11\n\
v12 = stack_slot.i64\n\
v13 = iadd v12, v1\n\
store v4, v12\n\
store v2, v13\n\
v14 = call %host_set_summary_file(v12)\n\
v15 = icmp eq v14, v5\n\
brif v15, block1, block_fail_set_summary\n\
block_fail_set_summary:\n\
return v6\n\
block1:\n\
v16 = stack_slot.i64\n\
v17 = call %host_emit_ir_from_compiler_state(v0, v16)\n\
v18 = icmp eq v17, v5\n\
brif v18, block2, block_fail_emit\n\
block_fail_emit:\n\
return v7\n\
block2:\n\
v19 = stack_slot.i64\n\
v20 = call %host_run_cranelift_aot(v16, v19)\n\
v21 = icmp eq v20, v5\n\
brif v21, block3, block_fail_run\n\
block_fail_run:\n\
return v8\n\
block3:\n\
v22 = call %host_link_executable_from_objects(v19, v10)\n\
v23 = icmp eq v22, v5\n\
brif v23, block4, block_fail_link\n\
block_fail_link:\n\
return v9\n\
block4:\n\
v24 = call %host_write_aot_cli_summary(v10, v16, v19)\n\
return v24\n\
}}\n",
            cc = call_conv
        );
        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path.clone()),
            ..AotCompileConfig::default()
        };
        let driver_object = temp_root.join("bridge_stage_driver.obj");
        compile_clif_to_object(&driver_clif, &driver_object, &compile_config)
            .expect("compile bridge driver object");

        let shim_clif = format!(
            "external bridge_stage_main() -> i32 {cc}\n\
external ExitProcess(i32) {cc}\n\
function %stasis_entry_shim() {cc} {{\n\
block0:\n\
v0 = call %bridge_stage_main()\n\
call %ExitProcess(v0)\n\
return\n\
}}\n",
            cc = call_conv
        );
        let shim_object = temp_root.join("bridge_stage_shim.obj");
        compile_clif_to_object(&shim_clif, &shim_object, &compile_config)
            .expect("compile bridge shim object");

        let driver_exe = temp_root.join("bridge_stage_driver.exe");
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        link_objects_to_executable(
            &[driver_object, bridge_object, shim_object],
            &driver_exe,
            "stasis_entry_shim",
            &link_config,
        )
        .expect("link bridge driver executable");

        let status = Command::new(&driver_exe)
            .current_dir(&temp_root)
            .env("STASIS_SELF_HOST_IR_BUNDLE_PATH", "i")
            .env("STASIS_SELF_HOST_OBJECT_BUNDLE_PATH", "j")
            .env("STASIS_SELF_HOST_LINK_TEMPLATE_EXE", "t")
            .env("STASIS_SELF_HOST_SUMMARY_TEMPLATE_FILE", "u")
            .status();
        match status {
            Ok(value) => {
                assert_eq!(
                    value.code(),
                    Some(0),
                    "staged runtime bridge host extern calls should all succeed"
                );
            }
            Err(error) if error.to_string().contains("blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(error) => panic!("failed to run staged runtime bridge driver: {error}"),
        }

        let linked_output = temp_root.join("o");
        let summary_output = temp_root.join("s");
        assert!(
            linked_output.exists(),
            "expected linked output copied by runtime bridge host_link_executable_from_objects"
        );
        assert!(
            summary_output.exists(),
            "expected summary output copied by runtime bridge host_write_aot_cli_summary"
        );
        assert_eq!(
            fs::read(&linked_output).expect("read linked output"),
            fs::read(&template_exe).expect("read template exe"),
            "linked output should match configured template executable"
        );
        assert_eq!(
            fs::read_to_string(&summary_output).expect("read summary output"),
            fs::read_to_string(&template_summary).expect("read template summary"),
            "summary output should match configured template summary"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_clif_staged_externs_smoke_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_RUNTIME_BRIDGE_CLIF_STAGED_EXTERN_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_self_host_runtime_bridge_clif_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let template_ir = temp_root.join("i");
        let template_object = temp_root.join("j");
        let template_exe = temp_root.join("t");
        let template_summary = temp_root.join("u");
        fs::write(&template_ir, "{\"entry\":\"main\"}\n").expect("write template ir bundle");
        fs::write(&template_object, "{\"entry\":\"main\"}\n")
            .expect("write template object bundle");
        fs::write(&template_exe, "template-exe").expect("write template exe");
        fs::write(&template_summary, "{\"source_file_count\":1}\n")
            .expect("write template summary");

        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path),
            ..AotCompileConfig::default()
        };
        let backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config.clone(),
            AotLinkConfig {
                linker_path: Some(linker_path.clone()),
            },
            temp_root.join("artifacts"),
            false,
        );
        let bridge_object = temp_root.join("self_host_runtime_bridge_clif.obj");
        emit_self_host_runtime_bridge_object_clif(&backend, &bridge_object)
            .expect("emit clif runtime bridge object");

        let call_conv = aot_call_conv();
        let driver_clif = format!(
            "external host_set_summary_file(i64) -> i32 {cc}\n\
external host_emit_ir_from_compiler_state(i64, i64) -> i32 {cc}\n\
external host_run_cranelift_aot(i64, i64) -> i32 {cc}\n\
external host_link_executable_from_objects(i64, i64) -> i32 {cc}\n\
external host_write_aot_cli_summary(i64, i64, i64) -> i32 {cc}\n\
function %bridge_stage_main() -> i32 {cc} {{\n\
block0:\n\
v0 = iconst.i64 0\n\
v1 = iconst.i64 1\n\
v2 = iconst.i8 0\n\
v3 = iconst.i8 111\n\
v4 = iconst.i8 115\n\
v5 = iconst.i32 0\n\
v6 = iconst.i32 10\n\
v7 = iconst.i32 20\n\
v8 = iconst.i32 30\n\
v9 = iconst.i32 40\n\
v10 = stack_slot.i64\n\
v11 = iadd v10, v1\n\
store v3, v10\n\
store v2, v11\n\
v12 = stack_slot.i64\n\
v13 = iadd v12, v1\n\
store v4, v12\n\
store v2, v13\n\
v14 = call %host_set_summary_file(v12)\n\
v15 = icmp eq v14, v5\n\
brif v15, block1, block_fail_set_summary\n\
block_fail_set_summary:\n\
return v6\n\
block1:\n\
v16 = stack_slot.i64\n\
v17 = call %host_emit_ir_from_compiler_state(v0, v16)\n\
v18 = icmp eq v17, v5\n\
brif v18, block2, block_fail_emit\n\
block_fail_emit:\n\
return v7\n\
block2:\n\
v19 = stack_slot.i64\n\
v20 = call %host_run_cranelift_aot(v16, v19)\n\
v21 = icmp eq v20, v5\n\
brif v21, block3, block_fail_run\n\
block_fail_run:\n\
return v8\n\
block3:\n\
v22 = call %host_link_executable_from_objects(v19, v10)\n\
v23 = icmp eq v22, v5\n\
brif v23, block4, block_fail_link\n\
block_fail_link:\n\
return v9\n\
block4:\n\
v24 = call %host_write_aot_cli_summary(v10, v16, v19)\n\
return v24\n\
}}\n",
            cc = call_conv
        );
        let driver_object = temp_root.join("bridge_stage_driver_clif.obj");
        compile_clif_to_object(&driver_clif, &driver_object, &compile_config)
            .expect("compile bridge driver object");

        let shim_clif = format!(
            "external bridge_stage_main() -> i32 {cc}\n\
external ExitProcess(i32) {cc}\n\
function %stasis_entry_shim() {cc} {{\n\
block0:\n\
v0 = call %bridge_stage_main()\n\
call %ExitProcess(v0)\n\
return\n\
}}\n",
            cc = call_conv
        );
        let shim_object = temp_root.join("bridge_stage_shim_clif.obj");
        compile_clif_to_object(&shim_clif, &shim_object, &compile_config)
            .expect("compile bridge shim object");

        let driver_exe = temp_root.join("bridge_stage_driver_clif.exe");
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        link_objects_to_executable(
            &[driver_object, bridge_object, shim_object],
            &driver_exe,
            "stasis_entry_shim",
            &link_config,
        )
        .expect("link bridge driver executable");

        let status = Command::new(&driver_exe)
            .current_dir(&temp_root)
            .env("STASIS_SELF_HOST_IR_BUNDLE_PATH", "i")
            .env("STASIS_SELF_HOST_OBJECT_BUNDLE_PATH", "j")
            .env("STASIS_SELF_HOST_LINK_TEMPLATE_EXE", "t")
            .env("STASIS_SELF_HOST_SUMMARY_TEMPLATE_FILE", "u")
            .status();
        match status {
            Ok(value) => {
                assert_eq!(
                    value.code(),
                    Some(0),
                    "clif fallback staged runtime bridge host extern calls should all succeed"
                );
            }
            Err(error) if error.to_string().contains("blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(error) => panic!("failed to run clif staged runtime bridge driver: {error}"),
        }

        let linked_output = temp_root.join("o");
        let summary_output = temp_root.join("s");
        assert!(
            linked_output.exists(),
            "expected linked output copied by clif runtime bridge host_link_executable_from_objects"
        );
        assert!(
            summary_output.exists(),
            "expected summary output copied by clif runtime bridge host_write_aot_cli_summary"
        );
        assert_eq!(
            fs::read(&linked_output).expect("read linked output"),
            fs::read(&template_exe).expect("read template exe"),
            "linked output should match configured template executable"
        );
        assert_eq!(
            fs::read_to_string(&summary_output).expect("read summary output"),
            fs::read_to_string(&template_summary).expect("read template summary"),
            "summary output should match configured template summary"
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn self_host_runtime_bridge_clif_arg_and_source_externs_smoke_if_real_toolchain_available() {
        let _process_env_guard = stasis_process_env_lock().lock().expect("lock process env");
        let run_smoke = std::env::var("STASIS_RUN_REAL_RUNTIME_BRIDGE_CLIF_ARG_SOURCE_SMOKE")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !run_smoke {
            return;
        }

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

        let helper_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        if !helper_path.exists() {
            return;
        }
        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .join(format!("stasis_self_host_runtime_bridge_clif_arg_source_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let compile_config = AotCompileConfig {
            helper_path: Some(helper_path),
            ..AotCompileConfig::default()
        };
        let backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config.clone(),
            AotLinkConfig {
                linker_path: Some(linker_path.clone()),
            },
            temp_root.join("artifacts"),
            false,
        );
        let bridge_object = temp_root.join("self_host_runtime_bridge_clif.obj");
        emit_self_host_runtime_bridge_object_clif(&backend, &bridge_object)
            .expect("emit clif runtime bridge object");

        let call_conv = aot_call_conv();
        let driver_clif = format!(
            "external host_cli_arg_count() -> i32 {cc}\n\
external host_cli_arg_value(i32, i64) -> i32 {cc}\n\
external host_source_file_count(i64) -> i32 {cc}\n\
external host_load_source_file(i64, i32, i64, i64) -> i32 {cc}\n\
function %bridge_arg_source_main() -> i32 {cc} {{\n\
block0:\n\
v0 = iconst.i32 0\n\
v1 = iconst.i32 1\n\
v2 = iconst.i32 10\n\
v3 = iconst.i32 20\n\
v4 = iconst.i32 30\n\
v5 = iconst.i32 40\n\
v6 = iconst.i32 50\n\
v7 = iconst.i32 60\n\
v8 = iconst.i32 112\n\
v9 = iconst.i32 113\n\
v10 = iconst.i32 114\n\
v11 = iconst.i64 0\n\
v12 = call %host_cli_arg_count()\n\
v13 = icmp eq v12, v1\n\
brif v13, block1, block_fail_arg_count\n\
block_fail_arg_count:\n\
return v2\n\
block1:\n\
v14 = stack_slot.i64\n\
v15 = call %host_cli_arg_value(v0, v14)\n\
v16 = icmp eq v15, v0\n\
brif v16, block2, block_fail_arg_value\n\
block_fail_arg_value:\n\
return v3\n\
block2:\n\
v17 = load.i8 v14\n\
v18 = uextend.i32 v17\n\
v19 = icmp eq v18, v8\n\
brif v19, block3, block_fail_arg_data\n\
block_fail_arg_data:\n\
return v4\n\
block3:\n\
v20 = call %host_source_file_count(v11)\n\
v21 = icmp eq v20, v1\n\
brif v21, block4, block_fail_source_count\n\
block_fail_source_count:\n\
return v5\n\
block4:\n\
v22 = stack_slot.i64\n\
v23 = stack_slot.i64\n\
v24 = call %host_load_source_file(v11, v0, v22, v23)\n\
v25 = icmp eq v24, v0\n\
brif v25, block5, block_fail_source_load\n\
block_fail_source_load:\n\
return v6\n\
block5:\n\
v26 = load.i8 v22\n\
v27 = uextend.i32 v26\n\
v28 = icmp eq v27, v9\n\
brif v28, block6, block_fail_source_path\n\
block_fail_source_path:\n\
return v7\n\
block6:\n\
v29 = load.i8 v23\n\
v30 = uextend.i32 v29\n\
v31 = icmp eq v30, v10\n\
brif v31, block_ok, block_fail_source_text\n\
block_fail_source_text:\n\
return v7\n\
block_ok:\n\
return v0\n\
}}\n",
            cc = call_conv
        );
        let driver_object = temp_root.join("bridge_arg_source_driver_clif.obj");
        compile_clif_to_object(&driver_clif, &driver_object, &compile_config)
            .expect("compile bridge arg/source driver object");

        let shim_clif = format!(
            "external bridge_arg_source_main() -> i32 {cc}\n\
external ExitProcess(i32) {cc}\n\
function %stasis_entry_shim() {cc} {{\n\
block0:\n\
v0 = call %bridge_arg_source_main()\n\
call %ExitProcess(v0)\n\
return\n\
}}\n",
            cc = call_conv
        );
        let shim_object = temp_root.join("bridge_arg_source_shim_clif.obj");
        compile_clif_to_object(&shim_clif, &shim_object, &compile_config)
            .expect("compile bridge arg/source shim object");

        let driver_exe = temp_root.join("bridge_arg_source_driver_clif.exe");
        let link_config = AotLinkConfig {
            linker_path: Some(linker_path),
        };
        link_objects_to_executable(
            &[driver_object, bridge_object, shim_object],
            &driver_exe,
            "stasis_entry_shim",
            &link_config,
        )
        .expect("link bridge arg/source driver executable");

        let status = Command::new(&driver_exe)
            .env("STASIS_SELF_HOST_ARG_COUNT", "1")
            .env("STASIS_SELF_HOST_ARG_0", "p")
            .env("STASIS_SELF_HOST_SOURCE_FILE_COUNT", "1")
            .env("STASIS_SELF_HOST_SOURCE_PATH_0", "q")
            .env("STASIS_SELF_HOST_SOURCE_TEXT_0", "r")
            .status();
        match status {
            Ok(value) => {
                assert_eq!(
                    value.code(),
                    Some(0),
                    "clif fallback runtime bridge arg/source extern calls should all succeed"
                );
            }
            Err(error) if error.to_string().contains("blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(error) => panic!("failed to run clif arg/source bridge driver: {error}"),
        }

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

fn parse_project_import_paths(source: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("import ") {
            continue;
        }
        let Some(first_quote) = trimmed.find('"') else {
            continue;
        };
        let rest = &trimmed[first_quote + 1..];
        let Some(second_quote_rel) = rest.find('"') else {
            continue;
        };
        let candidate = &rest[..second_quote_rel];
        let path = PathBuf::from(candidate);
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
        {
            out.push(path);
        }
    }
    out
}

fn collect_stasis_files_for_self_host_project(root: &Path) -> Result<Vec<PathBuf>, String> {
    let entry_file = std::env::var("STASIS_AOT_ENTRY_FILE")
        .ok()
        .and_then(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(value))
            }
        });
    collect_stasis_files_for_self_host_project_with_entry(root, entry_file.as_deref())
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

    let mut queue: Vec<PathBuf> = vec![entry_canonical];
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some(path) = queue.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let parent = path.parent().unwrap_or(&root_canonical);
        for import_path in parse_project_import_paths(&source) {
            let candidate = parent.join(import_path);
            if !candidate.exists() {
                continue;
            }
            let canonical = candidate.canonicalize().map_err(|error| {
                format!("failed to canonicalize {}: {error}", candidate.display())
            })?;
            if canonical.starts_with(&root_canonical) {
                queue.push(canonical);
            }
        }
    }

    let mut files: Vec<PathBuf> = visited.into_iter().collect();
    files.sort();
    Ok(files)
}

fn format_compile_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "compile failed without diagnostics".to_string();
    }
    diagnostics
        .iter()
        .map(|diag| {
            let path = diag
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            let line = diag.line.unwrap_or(0);
            let column = diag.column.unwrap_or(0);
            format!(
                "{:?}: {} ({path}:{line}:{column})",
                diag.severity, diag.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn host_emit_ir_from_compiler_state_with_backend(
    backend: &mut IncrementalCompilerBackend,
    project_dir: &Path,
    env_snapshot: &SelfHostCliEnvSnapshot,
) -> Result<PathBuf, String> {
    let changed_files = collect_stasis_files_for_self_host_project(project_dir)?;
    if changed_files.is_empty() {
        return Err(format!(
            "no .stasis files found under {}",
            project_dir.display()
        ));
    }
    let request = CompileRequest::new(RequestId(1), changed_files.clone(), TargetMode::AotProd);
    let result = backend.compile(request);
    if result.status == stasis_runner::swap::contracts::CompileStatus::Failed {
        let entry_override_missing = std::env::var("STASIS_AOT_ENTRY_FILE")
            .ok()
            .is_none_or(|value| value.trim().is_empty());
        let multiple_main_error = result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("multiple main declarations (code=43"));
        if entry_override_missing && multiple_main_error {
            return Err(format!(
                "{}\nHint: this project has multiple main() files; rerun with --entry-file <relative/path/to_entry.stasis>",
                format_compile_diagnostics(&result.diagnostics)
            ));
        }
        return Err(format_compile_diagnostics(&result.diagnostics));
    }
    let main_fn_id = backend
        .existing_fn_id_for_identifier_hash(hash_identifier("main"))
        .ok_or_else(|| "missing resolved fn_id for main".to_string())?;
    let entry_symbol = result
        .aot_function_symbols
        .as_ref()
        .and_then(|symbols| {
            symbols
                .iter()
                .find(|entry| entry.fn_id == main_fn_id)
                .map(|entry| entry.symbol.clone())
        })
        .ok_or_else(|| "missing AOT symbol mapping for main".to_string())?;

    let manifest_path = backend.aot_artifact_root.join("last_patch_manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "failed to read AOT patch manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: AotPatchManifest = serde_json::from_str(&manifest_text).map_err(|error| {
        format!(
            "failed to parse AOT patch manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    if env_snapshot.strict_self_host
        && !env_snapshot.allow_stub_fallback
        && !manifest.fallback_stub_symbols.is_empty()
    {
        let preview = manifest
            .fallback_stub_symbols
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "self-host aot-cli strict mode rejected stub fallback lowering for i32 functions: {preview}; set STASIS_AOT_ALLOW_STUB_FALLBACK=1 to bypass temporarily"
        ));
    }
    if env_snapshot.quality_gate
        && manifest
            .fallback_stub_symbols
            .iter()
            .any(|symbol| symbol == &entry_symbol)
    {
        return Err(format!(
            "quality gate rejected output: entry symbol {entry_symbol} still uses fallback stub lowering; add lowering coverage for selected entry path before producing game executable"
        ));
    }
    let ir_bundle = SelfHostIrBundle {
        source_file_count: changed_files.len(),
        entry_symbol,
        object_paths: manifest.artifact_paths,
    };
    let ir_bundle_path = backend.aot_artifact_root.join("self_host_ir_bundle.json");
    let ir_json = serde_json::to_string_pretty(&ir_bundle)
        .map_err(|error| format!("failed to serialize ir bundle metadata: {error}"))?;
    std::fs::write(&ir_bundle_path, ir_json).map_err(|error| {
        format!(
            "failed to write ir bundle metadata {}: {error}",
            ir_bundle_path.display()
        )
    })?;
    Ok(ir_bundle_path)
}

fn host_run_cranelift_aot_from_ir_bundle(ir_bundle_path: &Path) -> Result<PathBuf, String> {
    let ir_text = std::fs::read_to_string(ir_bundle_path).map_err(|error| {
        format!(
            "failed to read ir bundle {}: {error}",
            ir_bundle_path.display()
        )
    })?;
    let ir_bundle: SelfHostIrBundle = serde_json::from_str(&ir_text).map_err(|error| {
        format!(
            "failed to parse ir bundle {}: {error}",
            ir_bundle_path.display()
        )
    })?;
    if ir_bundle.object_paths.is_empty() {
        return Err("ir bundle contained no object paths".to_string());
    }
    for object in &ir_bundle.object_paths {
        let object_path = Path::new(object);
        if !object_path.exists() {
            return Err(format!(
                "object path from ir bundle does not exist: {}",
                object_path.display()
            ));
        }
    }
    let object_bundle = SelfHostObjectBundle {
        entry_symbol: ir_bundle.entry_symbol,
        object_paths: ir_bundle.object_paths,
    };
    let object_bundle_path = ir_bundle_path.with_file_name("self_host_object_bundle.json");
    let object_json = serde_json::to_string_pretty(&object_bundle)
        .map_err(|error| format!("failed to serialize object bundle metadata: {error}"))?;
    std::fs::write(&object_bundle_path, object_json).map_err(|error| {
        format!(
            "failed to write object bundle metadata {}: {error}",
            object_bundle_path.display()
        )
    })?;
    Ok(object_bundle_path)
}

fn host_link_executable_from_object_bundle_with_backend(
    backend: &mut IncrementalCompilerBackend,
    object_bundle_path: &Path,
    output_exe: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    let object_text = std::fs::read_to_string(object_bundle_path).map_err(|error| {
        format!(
            "failed to read object bundle {}: {error}",
            object_bundle_path.display()
        )
    })?;
    let object_bundle: SelfHostObjectBundle =
        serde_json::from_str(&object_text).map_err(|error| {
            format!(
                "failed to parse object bundle {}: {error}",
                object_bundle_path.display()
            )
        })?;
    let mut object_paths: Vec<PathBuf> = object_bundle
        .object_paths
        .iter()
        .map(PathBuf::from)
        .collect();
    let (runtime_bridge_object, runtime_bridge_mode) =
        emit_self_host_runtime_bridge_object(backend)?;
    object_paths.push(runtime_bridge_object);
    let mut executable_entry_symbol = object_bundle.entry_symbol.clone();
    if cfg!(windows) {
        let shim_object = backend.aot_artifact_root.join("self_host_entry_shim.obj");
        let shim_clif = format!(
            "external {entry_symbol}() -> i32 {cc}\nexternal ExitProcess(i32) {cc}\nfunction %stasis_entry_shim() {cc} {{\nblock0:\nv0 = call %{entry_symbol}()\ncall %ExitProcess(v0)\nreturn\n}}\n",
            entry_symbol = object_bundle.entry_symbol,
            cc = aot_call_conv()
        );
        compile_clif_to_object(&shim_clif, &shim_object, &backend.aot_compile_config)?;
        object_paths.push(shim_object);
        executable_entry_symbol = "stasis_entry_shim".to_string();
    }
    let initial_link = link_objects_to_executable(
        &object_paths,
        output_exe,
        &executable_entry_symbol,
        &backend.aot_link_config,
    );
    if let Err(initial_error) = initial_link {
        if cfg!(windows) && runtime_bridge_mode == "rustc" {
            let bridge_object = backend
                .aot_artifact_root
                .join("self_host_runtime_bridge.obj");
            emit_self_host_runtime_bridge_object_clif(backend, &bridge_object)?;
            link_objects_to_executable(
                &object_paths,
                output_exe,
                &executable_entry_symbol,
                &backend.aot_link_config,
            )
            .map_err(|fallback_error| {
                format!(
                    "link failed with rustc runtime bridge and fallback clif bridge\nrustc_link_error:\n{initial_error}\nclif_link_error:\n{fallback_error}"
                )
            })?;
        } else {
            return Err(initial_error);
        }
    }
    maybe_sign_output_executable(output_exe)?;
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
        linked_image_path: output_exe.to_path_buf(),
        entry_symbol: object_bundle.entry_symbol,
        ir_bundle_path: PathBuf::new(),
        object_bundle_path: object_bundle_path.to_path_buf(),
        object_file_names,
    })
}

fn emit_self_host_runtime_bridge_object(
    backend: &IncrementalCompilerBackend,
) -> Result<(PathBuf, &'static str), String> {
    let bridge_object = backend
        .aot_artifact_root
        .join("self_host_runtime_bridge.obj");
    if cfg!(windows) {
        if let Ok(()) = emit_self_host_runtime_bridge_object_windows_rustc(&bridge_object) {
            write_runtime_bridge_mode_marker(backend, "rustc").ok();
            return Ok((bridge_object, "rustc"));
        }
    }
    emit_self_host_runtime_bridge_object_clif(backend, &bridge_object)?;
    Ok((bridge_object, "clif"))
}

fn emit_self_host_runtime_bridge_object_clif(
    backend: &IncrementalCompilerBackend,
    bridge_object: &Path,
) -> Result<(), String> {
    let cc = aot_call_conv();
    let bridge_clif = build_self_host_runtime_bridge_clif(cc);
    compile_clif_to_object(&bridge_clif, bridge_object, &backend.aot_compile_config)?;
    write_runtime_bridge_mode_marker(backend, "clif").ok();
    Ok(())
}

const CLIF_RUNTIME_BRIDGE_MAX_INDEXED_KEYS: usize = 128;

fn build_self_host_runtime_bridge_key_selector(
    fn_name: &str,
    global_prefix: &str,
    max_indexed_keys: usize,
    cc: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("function %{fn_name}(i32) -> i64 {cc} {{\n"));
    out.push_str("block0:\n");
    out.push_str("v1 = iconst.i32 0\n");
    out.push_str("v2 = iconst.i64 0\n");
    out.push_str("v3 = icmp slt v0, v1\n");
    if max_indexed_keys == 0 {
        out.push_str("brif v3, block_fail, block_fail\n");
    } else {
        out.push_str("brif v3, block_fail, block_check_0\n");
    }

    for index in 0..max_indexed_keys {
        let iconst_id = 10 + index * 3;
        let eq_id = iconst_id + 1;
        let key_id = iconst_id + 2;
        let next_block = if index + 1 < max_indexed_keys {
            format!("block_check_{}", index + 1)
        } else {
            "block_fail".to_string()
        };
        out.push_str(&format!("block_check_{index}:\n"));
        out.push_str(&format!("v{iconst_id} = iconst.i32 {index}\n"));
        out.push_str(&format!("v{eq_id} = icmp eq v0, v{iconst_id}\n"));
        out.push_str(&format!(
            "brif v{eq_id}, block_return_{index}, {next_block}\n"
        ));
        out.push_str(&format!("block_return_{index}:\n"));
        out.push_str(&format!("v{key_id} = global_value {global_prefix}{index}\n"));
        out.push_str(&format!("return v{key_id}\n"));
    }

    out.push_str("block_fail:\n");
    out.push_str("return v2\n");
    out.push_str("}\n");
    out
}

fn build_self_host_runtime_bridge_clif(cc: &str) -> String {
    // CLIF fallback runtime bridge for environments where rustc object emission fails.
    // Keep all host bridge externs process/env-backed so fallback remains executable-path compatible.
    let mut source = String::new();
    source.push_str("global k_arg_count_key: i8 ; \"STASIS_SELF_HOST_ARG_COUNT\"\n");
    source.push_str("global k_source_count_key: i8 ; \"STASIS_SELF_HOST_SOURCE_FILE_COUNT\"\n");
    source.push_str("global k_summary_key: i8 ; \"STASIS_AOT_SUMMARY_FILE\"\n");
    source.push_str("global k_ir_bundle_key: i8 ; \"STASIS_SELF_HOST_IR_BUNDLE_PATH\"\n");
    source.push_str("global k_object_bundle_key: i8 ; \"STASIS_SELF_HOST_OBJECT_BUNDLE_PATH\"\n");
    source.push_str("global k_link_template_exe_key: i8 ; \"STASIS_SELF_HOST_LINK_TEMPLATE_EXE\"\n");
    source.push_str(
        "global k_summary_template_key: i8 ; \"STASIS_SELF_HOST_SUMMARY_TEMPLATE_FILE\"\n",
    );
    source.push_str("global k_tmp_count_buf: i8[8]\n");
    source.push_str("global k_tmp_path_buf0: i8[1024]\n");
    source.push_str("global k_tmp_path_buf1: i8[1024]\n");
    source.push_str("global k_cli_project_dir: i8[1024]\n");
    source.push_str("global k_cli_output_exe: i8[1024]\n");
    source.push_str("global k_cli_summary_path: i8[1024]\n");
    source.push_str("global k_cli_ir_bundle: i8[1024]\n");
    source.push_str("global k_cli_object_bundle: i8[1024]\n");

    for index in 0..CLIF_RUNTIME_BRIDGE_MAX_INDEXED_KEYS {
        source.push_str(&format!(
            "global k_arg_key_{index}: i8 ; \"STASIS_SELF_HOST_ARG_{index}\"\n"
        ));
        source.push_str(&format!(
            "global k_source_path_key_{index}: i8 ; \"STASIS_SELF_HOST_SOURCE_PATH_{index}\"\n"
        ));
        source.push_str(&format!(
            "global k_source_text_key_{index}: i8 ; \"STASIS_SELF_HOST_SOURCE_TEXT_{index}\"\n"
        ));
    }

    source.push_str(
        &r#"external GetEnvironmentVariableA(i64, i64, i32) -> i32 {cc}
external SetEnvironmentVariableA(i64, i64) -> i32 {cc}
external CopyFileA(i64, i64, i32) -> i32 {cc}
function %print_i32(i32) {cc} {
block0:
return
}
function %print_string(i64) {cc} {
block0:
return
}
function %parse_u32_ascii_from_buffer(i64) -> i32 {cc} {
block0:
v1 = stack_slot.i64
v2 = stack_slot.i64
v3 = iconst.i32 0
store v3, v1
store v3, v2
jump block_loop
block_loop:
v4 = load.i32 v1
v5 = sextend.i64 v4
v6 = iadd v0, v5
v7 = load.i8 v6
v8 = uextend.i32 v7
v9 = icmp eq v8, v3
brif v9, block_done, block_check_digit
block_check_digit:
v10 = iconst.i32 48
v11 = iconst.i32 57
v12 = icmp ult v8, v10
v13 = icmp ugt v8, v11
v14 = bor v12, v13
brif v14, block_fail, block_update
block_update:
v15 = load.i32 v2
v16 = iconst.i32 10
v17 = imul v15, v16
v18 = isub v8, v10
v19 = iadd v17, v18
store v19, v2
v20 = iconst.i32 1
v21 = iadd v4, v20
store v21, v1
jump block_loop
block_done:
v22 = load.i32 v2
return v22
block_fail:
return v3
}
function %read_env_count_i32(i64) -> i32 {cc} {
block0:
v1 = global_value k_tmp_count_buf
v2 = iconst.i32 8
v3 = call %GetEnvironmentVariableA(v0, v1, v2)
v4 = iconst.i32 0
v5 = icmp eq v3, v4
v6 = icmp uge v3, v2
v7 = bor v5, v6
brif v7, block_fail, block_parse
block_parse:
v8 = call %parse_u32_ascii_from_buffer(v1)
return v8
block_fail:
return v4
}
function %read_env_ascii_to_out(i64, i64, i32) -> i32 {cc} {
block0:
v3 = iconst.i32 0
v4 = iconst.i32 1
v5 = iconst.i64 0
v6 = icmp eq v1, v5
brif v6, block_fail, block_read
block_read:
v7 = call %GetEnvironmentVariableA(v0, v1, v2)
v8 = icmp eq v7, v3
v9 = icmp uge v7, v2
v10 = bor v8, v9
brif v10, block_fail, block_ok
block_fail:
return v4
block_ok:
return v3
}
function %copy_file_ascii(i64, i64) -> i32 {cc} {
block0:
v2 = iconst.i32 0
v3 = iconst.i32 1
v4 = iconst.i64 0
v5 = icmp eq v0, v4
v6 = icmp eq v1, v4
v7 = bor v5, v6
brif v7, block_fail, block_copy
block_copy:
v8 = call %CopyFileA(v0, v1, v2)
v9 = icmp eq v8, v2
brif v9, block_fail, block_ok
block_fail:
return v3
block_ok:
return v2
}
"#
        .replace("{cc}", cc),
    );

    source.push_str(&build_self_host_runtime_bridge_key_selector(
        "select_arg_key",
        "k_arg_key_",
        CLIF_RUNTIME_BRIDGE_MAX_INDEXED_KEYS,
        cc,
    ));
    source.push_str(&build_self_host_runtime_bridge_key_selector(
        "select_source_path_key",
        "k_source_path_key_",
        CLIF_RUNTIME_BRIDGE_MAX_INDEXED_KEYS,
        cc,
    ));
    source.push_str(&build_self_host_runtime_bridge_key_selector(
        "select_source_text_key",
        "k_source_text_key_",
        CLIF_RUNTIME_BRIDGE_MAX_INDEXED_KEYS,
        cc,
    ));

    source.push_str(
        &r#"function %host_cli_arg_count() -> i32 {cc} {
block0:
v1 = global_value k_arg_count_key
v2 = call %read_env_count_i32(v1)
return v2
}
function %host_cli_arg_value(i32, i64) -> i32 {cc} {
block0:
v2 = iconst.i32 0
v3 = iconst.i32 1
v4 = iconst.i64 0
v5 = icmp slt v0, v2
v6 = icmp eq v1, v4
v7 = bor v5, v6
brif v7, block_fail, block_select
block_select:
v8 = call %select_arg_key(v0)
v9 = icmp eq v8, v4
brif v9, block_fail, block_read
block_read:
v10 = iconst.i32 1024
v11 = call %read_env_ascii_to_out(v8, v1, v10)
v12 = icmp eq v11, v2
brif v12, block_ok, block_fail
block_fail:
return v3
block_ok:
return v2
}
function %host_set_summary_file(i64) -> i32 {cc} {
block0:
v10 = global_value k_summary_key
v11 = iconst.i32 0
v12 = call %SetEnvironmentVariableA(v10, v0)
v13 = icmp eq v12, v11
brif v13, block_fail, block_ok
block_fail:
v14 = iconst.i32 1
return v14
block_ok:
return v11
}
function %host_source_file_count(i64) -> i32 {cc} {
block0:
v1 = global_value k_source_count_key
v2 = call %read_env_count_i32(v1)
return v2
}
function %host_load_source_file(i64, i32, i64, i64) -> i32 {cc} {
block0:
v10 = iconst.i32 0
v11 = iconst.i32 1
v12 = iconst.i64 0
v13 = icmp slt v1, v10
v14 = icmp eq v2, v12
v15 = icmp eq v3, v12
v16 = bor v13, v14
v17 = bor v16, v15
brif v17, block_fail, block_keys
block_keys:
v18 = call %select_source_path_key(v1)
v19 = call %select_source_text_key(v1)
v20 = icmp eq v18, v12
v21 = icmp eq v19, v12
v22 = bor v20, v21
brif v22, block_fail, block_read_path
block_read_path:
v23 = iconst.i32 1024
v24 = call %read_env_ascii_to_out(v18, v2, v23)
v25 = icmp eq v24, v10
brif v25, block_read_source, block_fail
block_read_source:
v26 = iconst.i32 262144
v27 = call %read_env_ascii_to_out(v19, v3, v26)
v28 = icmp eq v27, v10
brif v28, block_ok, block_fail
block_fail:
return v11
block_ok:
return v10
}
function %host_emit_ir_from_compiler_state(i64, i64) -> i32 {cc} {
block0:
v10 = global_value k_ir_bundle_key
v11 = iconst.i32 1024
v12 = call %read_env_ascii_to_out(v10, v1, v11)
return v12
}
function %host_run_cranelift_aot(i64, i64) -> i32 {cc} {
block0:
v10 = global_value k_object_bundle_key
v11 = iconst.i32 1024
v12 = call %read_env_ascii_to_out(v10, v1, v11)
return v12
}
function %host_link_executable_from_objects(i64, i64) -> i32 {cc} {
block0:
v10 = iconst.i32 0
v11 = iconst.i32 1
v12 = global_value k_tmp_path_buf0
v13 = global_value k_link_template_exe_key
v14 = iconst.i32 1024
v15 = call %read_env_ascii_to_out(v13, v12, v14)
v16 = icmp eq v15, v10
brif v16, block_copy, block_fail
block_copy:
v17 = call %copy_file_ascii(v12, v1)
return v17
block_fail:
return v11
}
function %host_write_aot_cli_summary(i64, i64, i64) -> i32 {cc} {
block0:
v10 = iconst.i32 1024
v11 = iconst.i32 0
v12 = iconst.i32 1
v13 = global_value k_tmp_path_buf0
v14 = global_value k_summary_key
v15 = call %GetEnvironmentVariableA(v14, v13, v10)
v16 = icmp eq v15, v11
brif v16, block_none, block_have_summary
block_none:
return v11
block_have_summary:
v17 = icmp uge v15, v10
brif v17, block_fail, block_read_template
block_read_template:
v18 = global_value k_tmp_path_buf1
v19 = global_value k_summary_template_key
v20 = call %read_env_ascii_to_out(v19, v18, v10)
v21 = icmp eq v20, v11
brif v21, block_copy, block_fail
block_copy:
v22 = call %copy_file_ascii(v18, v13)
return v22
block_fail:
return v12
}
function %host_run_self_host_aot_cli_from_env() -> i32 {cc} {
block0:
v0 = iconst.i32 0
v1 = iconst.i32 4
v2 = iconst.i32 6
v3 = iconst.i32 1
v4 = iconst.i32 3
v5 = iconst.i32 5
v6 = iconst.i32 20
v7 = iconst.i32 21
v8 = iconst.i32 22
v9 = iconst.i32 23
v10 = iconst.i32 24
v11 = iconst.i32 25
v12 = iconst.i32 26
v13 = iconst.i32 27
v14 = iconst.i32 28
v15 = call %host_cli_arg_count()
v16 = icmp slt v15, v1
brif v16, block_fail_argc, block_load_project
block_fail_argc:
return v6
block_load_project:
v17 = global_value k_cli_project_dir
v18 = call %host_cli_arg_value(v3, v17)
v19 = icmp eq v18, v0
brif v19, block_load_output, block_fail_project
block_fail_project:
return v7
block_load_output:
v20 = global_value k_cli_output_exe
v21 = call %host_cli_arg_value(v4, v20)
v22 = icmp eq v21, v0
brif v22, block_summary_check, block_fail_output
block_fail_output:
return v8
block_summary_check:
v23 = icmp slt v15, v2
brif v23, block_emit_ir, block_load_summary
block_load_summary:
v24 = global_value k_cli_summary_path
v25 = call %host_cli_arg_value(v5, v24)
v26 = icmp eq v25, v0
brif v26, block_set_summary, block_fail_summary_read
block_fail_summary_read:
return v9
block_set_summary:
v27 = call %host_set_summary_file(v24)
v28 = icmp eq v27, v0
brif v28, block_emit_ir, block_fail_summary_set
block_fail_summary_set:
return v10
block_emit_ir:
v29 = global_value k_cli_ir_bundle
v30 = call %host_emit_ir_from_compiler_state(v17, v29)
v31 = icmp eq v30, v0
brif v31, block_run_aot, block_fail_emit_ir
block_fail_emit_ir:
return v11
block_run_aot:
v32 = global_value k_cli_object_bundle
v33 = call %host_run_cranelift_aot(v29, v32)
v34 = icmp eq v33, v0
brif v34, block_link, block_fail_run_aot
block_fail_run_aot:
return v12
block_link:
v35 = call %host_link_executable_from_objects(v32, v20)
v36 = icmp eq v35, v0
brif v36, block_write_summary, block_fail_link
block_fail_link:
return v13
block_write_summary:
v37 = call %host_write_aot_cli_summary(v20, v29, v32)
v38 = icmp eq v37, v0
brif v38, block_ok, block_fail_summary_write
block_fail_summary_write:
return v14
block_ok:
return v0
}
"#
        .replace("{cc}", cc),
    );

    source
}

fn write_runtime_bridge_mode_marker(
    backend: &IncrementalCompilerBackend,
    mode: &str,
) -> Result<(), String> {
    let marker_path = backend
        .aot_artifact_root
        .join("self_host_runtime_bridge.mode");
    std::fs::write(&marker_path, mode).map_err(|error| {
        format!(
            "failed to write runtime bridge mode marker {}: {error}",
            marker_path.display()
        )
    })
}

fn emit_self_host_runtime_bridge_object_windows_rustc(output_object: &Path) -> Result<(), String> {
    if let Some(parent) = output_object.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create runtime bridge object directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let source_path = output_object.with_extension("rs");
    let source = r#"#![no_std]
#![allow(non_snake_case)]

use core::ffi::c_char;
use core::panic::PanicInfo;

const ARG_COUNT_KEY: &[u8] = b"STASIS_SELF_HOST_ARG_COUNT\0";
const ARG_PREFIX: &[u8] = b"STASIS_SELF_HOST_ARG_\0";
const SUMMARY_KEY: &[u8] = b"STASIS_AOT_SUMMARY_FILE\0";
const SOURCE_COUNT_KEY: &[u8] = b"STASIS_SELF_HOST_SOURCE_FILE_COUNT\0";
const SOURCE_PATH_PREFIX: &[u8] = b"STASIS_SELF_HOST_SOURCE_PATH_\0";
const SOURCE_TEXT_PREFIX: &[u8] = b"STASIS_SELF_HOST_SOURCE_TEXT_\0";
const IR_BUNDLE_PATH_KEY: &[u8] = b"STASIS_SELF_HOST_IR_BUNDLE_PATH\0";
const OBJECT_BUNDLE_PATH_KEY: &[u8] = b"STASIS_SELF_HOST_OBJECT_BUNDLE_PATH\0";
const LINK_TEMPLATE_EXE_KEY: &[u8] = b"STASIS_SELF_HOST_LINK_TEMPLATE_EXE\0";
const SUMMARY_TEMPLATE_FILE_KEY: &[u8] = b"STASIS_SELF_HOST_SUMMARY_TEMPLATE_FILE\0";

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetEnvironmentVariableA(lpName: *const c_char, lpBuffer: *mut c_char, nSize: u32) -> u32;
    fn SetEnvironmentVariableA(lpName: *const c_char, lpValue: *const c_char) -> i32;
    fn CopyFileA(lpExistingFileName: *const c_char, lpNewFileName: *const c_char, bFailIfExists: i32) -> i32;
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {}
}

fn parse_u32_ascii(buf: &[u8]) -> i32 {
    let mut value: i32 = 0;
    let mut i: usize = 0;
    while i < buf.len() {
        let b = buf[i];
        if b == 0 {
            break;
        }
        if b < b'0' || b > b'9' {
            return 0;
        }
        value = value.saturating_mul(10).saturating_add((b - b'0') as i32);
        i += 1;
    }
    value
}

fn read_env_ascii(key: &[u8], out_ptr: *mut u8, out_len: u32) -> u32 {
    unsafe { GetEnvironmentVariableA(key.as_ptr() as *const c_char, out_ptr as *mut c_char, out_len) }
}

fn read_env_ascii_to_out(key: &[u8], out_ptr: *mut u8, out_len: u32) -> i32 {
    if out_ptr.is_null() || out_len == 0 {
        return 1;
    }
    let written = read_env_ascii(key, out_ptr, out_len);
    if written == 0 || written >= out_len {
        return 1;
    }
    0
}

fn copy_file_ascii(src: *const u8, dst: *const u8) -> i32 {
    if src.is_null() || dst.is_null() {
        return 1;
    }
    let copied = unsafe { CopyFileA(src as *const c_char, dst as *const c_char, 0) };
    if copied == 0 { 1 } else { 0 }
}

fn write_indexed_key(prefix: &[u8], index: i32, out: &mut [u8]) -> bool {
    if index < 0 {
        return false;
    }
    let mut p: usize = 0;
    while p < prefix.len() - 1 {
        if p >= out.len() {
            return false;
        }
        out[p] = prefix[p];
        p += 1;
    }
    let mut digits = [0u8; 16];
    let mut count: usize = 0;
    let mut n = index as u32;
    if n == 0 {
        digits[0] = b'0';
        count = 1;
    } else {
        while n > 0 && count < digits.len() {
            digits[count] = b'0' + (n % 10) as u8;
            n /= 10;
            count += 1;
        }
    }
    let mut d = count;
    while d > 0 {
        d -= 1;
        if p + 1 >= out.len() {
            return false;
        }
        out[p] = digits[d];
        p += 1;
    }
    out[p] = 0;
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn print_i32(_value: i32) {}

#[unsafe(no_mangle)]
pub extern "system" fn print_string(_value: *const u8) {}

#[unsafe(no_mangle)]
pub extern "system" fn host_cli_arg_count() -> i32 {
    let mut buf = [0u8; 16];
    let written = read_env_ascii(ARG_COUNT_KEY, buf.as_mut_ptr(), 16);
    if written == 0 {
        return 0;
    }
    parse_u32_ascii(&buf)
}

#[unsafe(no_mangle)]
pub extern "system" fn host_cli_arg_value(index: i32, out_value: *mut u8) -> i32 {
    if index < 0 || out_value.is_null() {
        return 1;
    }
    let mut key = [0u8; 64];
    if !write_indexed_key(ARG_PREFIX, index, &mut key) {
        return 1;
    }
    let written = read_env_ascii(&key, out_value, 1024);
    if written == 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "system" fn host_set_summary_file(summary_file: *const u8) -> i32 {
    if summary_file.is_null() {
        return 1;
    }
    let ok = unsafe {
        SetEnvironmentVariableA(
            SUMMARY_KEY.as_ptr() as *const c_char,
            summary_file as *const c_char,
        )
    };
    if ok == 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "system" fn host_source_file_count(_project_dir: *const u8) -> i32 {
    let mut buf = [0u8; 16];
    let written = read_env_ascii(SOURCE_COUNT_KEY, buf.as_mut_ptr(), 16);
    if written == 0 {
        return 0;
    }
    parse_u32_ascii(&buf)
}
#[unsafe(no_mangle)]
pub extern "system" fn host_load_source_file(
    _project_dir: *const u8,
    file_index: i32,
    out_path: *mut u8,
    out_source: *mut u8
) -> i32 {
    if out_path.is_null() || out_source.is_null() {
        return 1;
    }
    let mut path_key = [0u8; 96];
    if !write_indexed_key(SOURCE_PATH_PREFIX, file_index, &mut path_key) {
        return 1;
    }
    let mut source_key = [0u8; 96];
    if !write_indexed_key(SOURCE_TEXT_PREFIX, file_index, &mut source_key) {
        return 1;
    }
    let path_written = read_env_ascii(&path_key, out_path, 1024);
    if path_written == 0 {
        return 1;
    }
    let source_written = read_env_ascii(&source_key, out_source, 262144);
    if source_written == 0 {
        return 1;
    }
    0
}
pub extern "system" fn host_emit_ir_from_compiler_state(_project_dir: *const u8, out_ir_bundle: *mut u8) -> i32 {
    read_env_ascii_to_out(IR_BUNDLE_PATH_KEY, out_ir_bundle, 1024)
}
#[unsafe(no_mangle)]
pub extern "system" fn host_run_cranelift_aot(_ir_bundle: *const u8, out_object_bundle: *mut u8) -> i32 {
    read_env_ascii_to_out(OBJECT_BUNDLE_PATH_KEY, out_object_bundle, 1024)
}
#[unsafe(no_mangle)]
pub extern "system" fn host_link_executable_from_objects(_object_bundle: *const u8, output_exe: *const u8) -> i32 {
    let mut template_path = [0u8; 1024];
    let template_written = read_env_ascii(
        LINK_TEMPLATE_EXE_KEY,
        template_path.as_mut_ptr(),
        template_path.len() as u32,
    );
    if template_written == 0 || template_written >= template_path.len() as u32 {
        return 1;
    }
    copy_file_ascii(template_path.as_ptr(), output_exe)
}
#[unsafe(no_mangle)]
pub extern "system" fn host_write_aot_cli_summary(_output_exe: *const u8, _ir_bundle: *const u8, _object_bundle: *const u8) -> i32 {
    let mut summary_path = [0u8; 1024];
    let summary_written = read_env_ascii(
        SUMMARY_KEY,
        summary_path.as_mut_ptr(),
        summary_path.len() as u32,
    );
    if summary_written == 0 {
        return 0;
    }
    if summary_written >= summary_path.len() as u32 {
        return 1;
    }
    let mut template_path = [0u8; 1024];
    let template_written = read_env_ascii(
        SUMMARY_TEMPLATE_FILE_KEY,
        template_path.as_mut_ptr(),
        template_path.len() as u32,
    );
    if template_written == 0 || template_written >= template_path.len() as u32 {
        return 1;
    }
    copy_file_ascii(template_path.as_ptr(), summary_path.as_ptr())
}
#[unsafe(no_mangle)]
pub extern "system" fn host_run_self_host_aot_cli_from_env() -> i32 {
    let argc = host_cli_arg_count();
    if argc < 4 {
        return 20;
    }
    let mut project_dir = [0u8; 1024];
    if host_cli_arg_value(1, project_dir.as_mut_ptr()) != 0 {
        return 21;
    }
    let mut output_exe = [0u8; 1024];
    if host_cli_arg_value(3, output_exe.as_mut_ptr()) != 0 {
        return 22;
    }
    if argc >= 6 {
        let mut summary_file = [0u8; 1024];
        if host_cli_arg_value(5, summary_file.as_mut_ptr()) != 0 {
            return 23;
        }
        if host_set_summary_file(summary_file.as_ptr()) != 0 {
            return 24;
        }
    }
    let mut ir_bundle = [0u8; 1024];
    if host_emit_ir_from_compiler_state(project_dir.as_ptr(), ir_bundle.as_mut_ptr()) != 0 {
        return 25;
    }
    let mut object_bundle = [0u8; 1024];
    if host_run_cranelift_aot(ir_bundle.as_ptr(), object_bundle.as_mut_ptr()) != 0 {
        return 26;
    }
    if host_link_executable_from_objects(object_bundle.as_ptr(), output_exe.as_ptr()) != 0 {
        return 27;
    }
    if host_write_aot_cli_summary(output_exe.as_ptr(), ir_bundle.as_ptr(), object_bundle.as_ptr()) != 0 {
        return 28;
    }
    0
}
"#;
    std::fs::write(&source_path, source).map_err(|error| {
        format!(
            "failed to write runtime bridge source {}: {error}",
            source_path.display()
        )
    })?;
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(rustc)
        .arg("--crate-type")
        .arg("lib")
        .arg("-C")
        .arg("panic=abort")
        .arg("--emit")
        .arg("obj")
        .arg(&source_path)
        .arg("-o")
        .arg(output_object)
        .output()
        .map_err(|error| {
            format!(
                "failed to execute rustc for runtime bridge object {}: {error}",
                output_object.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "runtime bridge rustc compile failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !output_object.exists() {
        return Err(format!(
            "runtime bridge rustc compile succeeded but did not produce {}",
            output_object.display()
        ));
    }
    Ok(())
}

fn run_self_host_aot_cli_with_backend(
    backend: &mut IncrementalCompilerBackend,
    project_dir: &Path,
    output_exe: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    if !project_dir.exists() {
        return Err(format!(
            "project directory does not exist: {}",
            project_dir.display()
        ));
    }
    let env_snapshot = capture_self_host_cli_env_snapshot();
    let changed_files = collect_stasis_files_for_self_host_project(project_dir)?;
    if changed_files.is_empty() {
        return Err(format!(
            "no .stasis files found under {}",
            project_dir.display()
        ));
    }
    let ir_bundle_path =
        host_emit_ir_from_compiler_state_with_backend(backend, project_dir, &env_snapshot)?;
    let object_bundle_path = host_run_cranelift_aot_from_ir_bundle(&ir_bundle_path)?;
    let mut summary = host_link_executable_from_object_bundle_with_backend(
        backend,
        &object_bundle_path,
        output_exe,
    )?;
    summary.source_file_count = changed_files.len();
    summary.ir_bundle_path = ir_bundle_path;
    summary.object_bundle_path = object_bundle_path;
    write_default_aot_cli_summary_sidecar(&summary, env_snapshot.summary_file_path.as_deref())?;
    Ok(summary)
}

pub fn run_self_host_aot_cli(
    project_dir: &Path,
    output_exe: &Path,
) -> Result<SelfHostedAotCliSummary, String> {
    let artifact_root = output_exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".stasis_cache")
        .join("aot_cli");
    let mut backend = IncrementalCompilerBackend::new_self_host_aot_cli(artifact_root);
    run_self_host_aot_cli_with_backend(&mut backend, project_dir, output_exe)
}

fn maybe_sign_output_executable(output_exe: &Path) -> Result<(), String> {
    let Some(sign_tool) = std::env::var_os("STASIS_AOT_SIGN_TOOL") else {
        return Ok(());
    };
    let status = std::process::Command::new(&sign_tool)
        .arg(output_exe)
        .status()
        .map_err(|error| {
            format!(
                "failed to launch signer tool {:?} for {}: {error}",
                sign_tool,
                output_exe.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "signer tool {:?} failed for {} with status {:?}",
            sign_tool,
            output_exe.display(),
            status.code()
        ));
    }
    Ok(())
}

fn metric_uses_stub_fallback(metric: &stasis_compiler::FunctionMetric) -> bool {
    if metric.return_type != "i32" {
        return false;
    }
    metric.simple_i32_return_expr.is_none()
        && metric.simple_i32_return_call_target_id_hash.is_none()
        && metric
            .simple_i32_return_call_one_arg_target_id_hash
            .is_none()
        && metric
            .simple_i32_return_call_one_arg_arg_call_target_id_hash
            .is_none()
        && metric
            .simple_i32_return_two_call_left_target_id_hash
            .is_none()
        && metric
            .simple_i32_return_two_call_right_target_id_hash
            .is_none()
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
            "import \"./dep.stasis\";\nimport \"../../src/stdlib/stdlib.stasis\";\nfunction main(): i32 { return 0; }\n",
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

