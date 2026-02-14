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
struct AotPatchManifest {
    request_id: u64,
    artifact_paths: Vec<String>,
    linked_image_path: Option<String>,
    linked_image_size_bytes: Option<u64>,
    linked_image_sha256: Option<String>,
    #[serde(default)]
    fallback_stub_symbols: Vec<String>,
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
        CompileResult::success_with_hook_metadata(
            request.request_id,
            layout_hash,
            FunctionPatchSet { functions },
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
            if metric_uses_stub_fallback(metric) {
                fallback_stub_symbols.push(function_name.clone());
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
            let resolved_simple_one_arg_call_target =
                resolve_unique_i32_single_i32_arg_call_target_symbol_by_hash(
                    metric.simple_i32_return_call_one_arg_target_id_hash,
                    metrics,
                );
            if metric.simple_i32_return_call_one_arg_target_id_hash.is_some()
                && resolved_simple_one_arg_call_target.is_none()
            {
                return Err(format!(
                    "unresolved one-arg direct call target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
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
                resolve_unique_i32_single_i32_arg_call_target_symbol_by_hash(
                    metric.simple_void_print_i32_call_target_id_hash,
                    metrics,
                )
            } else if metric
                .simple_void_print_i32_call_one_arg_arg_call_target_id_hash
                .is_some()
            {
                resolve_unique_i32_single_i32_arg_call_target_symbol_by_hash(
                    metric.simple_void_print_i32_call_target_id_hash,
                    metrics,
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
            if metric.simple_i32_return_two_call_left_target_id_hash.is_some()
                && resolved_simple_two_call_left_target.is_none()
            {
                return Err(format!(
                    "unresolved two-call left target for emitted function {} (id_hash={})",
                    function_name, metric.id_hash
                ));
            }
            if metric.simple_i32_return_two_call_right_target_id_hash.is_some()
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
            let clif = build_aot_stub_clif(
                &function_name,
                &metric.return_type,
                metric.simple_i32_return_expr.as_ref(),
                metric.body_hash,
                resolved_simple_call_target.as_deref(),
                metric.simple_i32_return_call_add_delta,
                resolved_simple_one_arg_call_target.as_deref(),
                metric.simple_i32_return_call_one_arg_i32_literal,
                resolved_simple_one_arg_arg_call_target.as_deref(),
                resolved_simple_two_call_left_target.as_deref(),
                resolved_simple_two_call_right_target.as_deref(),
                metric.simple_i32_return_two_call_op_code,
                metric.simple_void_print_i32_literal,
                resolved_simple_void_print_call_target.as_deref(),
                resolved_simple_void_print_one_arg_arg_call_target.as_deref(),
                metric.simple_void_print_i32_call_add_delta,
            );
            compile_clif_to_object(&clif, &object_path, &self.aot_compile_config)?;
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
    ) -> Result<(), String> {
        let manifest_path = self.aot_artifact_root.join("last_patch_manifest.json");
        let manifest = AotPatchManifest {
            request_id,
            artifact_paths: artifact_paths.to_vec(),
            linked_image_path: linked_image_path.map(str::to_string),
            linked_image_size_bytes,
            linked_image_sha256: linked_image_sha256.map(str::to_string),
            fallback_stub_symbols: fallback_stub_symbols.to_vec(),
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
            if let (Some(arg_literal), Some(delta)) =
                (simple_void_print_i32_literal, simple_void_print_i32_call_add_delta)
            {
                let abs_delta = delta.abs();
                let op = if delta < 0 { "isub" } else { "iadd" };
                return format!(
                    "external print_i32(i32) {}\nexternal {call_target_symbol}(i32) -> i32 {}\nfunction %{function_name}() {} {{\nblock0:\nv0 = iconst.i32 {arg_literal}\nv1 = call %{call_target_symbol}(v0)\nv2 = iconst.i32 {abs_delta}\nv3 = {op} v1, v2\ncall %print_i32(v3)\nreturn\n}}\n",
                    aot_call_conv(),
                    aot_call_conv(),
                    aot_call_conv()
                );
            }
            if let Some(arg_call_target_symbol) = simple_void_print_i32_call_one_arg_arg_call_target_symbol {
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
    let mut matches = metrics
        .iter()
        .filter(|candidate| candidate.id_hash == target_id_hash && candidate.return_type == "i32");
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(aot_symbol_name(first))
}

fn resolve_unique_i32_single_i32_arg_call_target_symbol_by_hash(
    maybe_target_id_hash: Option<i32>,
    metrics: &[stasis_compiler::FunctionMetric],
) -> Option<String> {
    let target_id_hash = maybe_target_id_hash?;
    let mut matches = metrics.iter().filter(|candidate| {
        candidate.id_hash == target_id_hash
            && candidate.return_type == "i32"
            && candidate.param_count == 1
            && candidate.first_param_type_code == 1
    });
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(aot_symbol_name(first))
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
    fn aot_stub_uses_print_i32_call_with_direct_one_i32_arg_call_target_for_simple_void_metadata()
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
    fn aot_stub_uses_print_i32_call_with_direct_one_call_arg_call_target_for_simple_void_metadata(
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
    fn aot_compile_rejects_unresolved_one_arg_direct_call_target() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_aot_unresolved_one_arg_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("sample.stasis");
        fs::write(&source, "function main(): i32 { return missing(7); }\n")
            .expect("write source");

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
                diagnostic.message.contains(
                    "unresolved one-arg direct call argument target for emitted function",
                )
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

        let summary = match run_self_host_aot_cli_with_backend(&mut backend, &project_dir, &output_exe) {
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("self-host summary sidecar run should succeed: {message}"),
        };
        let sidecar_path = default_aot_cli_summary_sidecar_path(&output_exe);
        assert!(sidecar_path.exists(), "expected sidecar {}", sidecar_path.display());
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("self-host summary configured-path run should succeed: {message}"),
        };
        assert!(configured_summary.exists(), "expected configured summary path");
        let sidecar_text = fs::read_to_string(&configured_summary).expect("read configured summary");
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("self-host runtime bridge mode run should succeed: {message}"),
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_rejects_stub_fallback_by_default() {
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
                fs::remove_dir_all(&temp_root).ok();
                return;
            }
            Err(message) => panic!("allow-stub-fallback run should succeed: {message}"),
        }

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn self_host_aot_cli_is_deterministic_across_repeated_runs_with_same_source() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_self_host_aot_determinism_{stamp}"));
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

        let first = match run_self_host_aot_cli_with_backend(
            &mut backend_first,
            &project_dir,
            &output_exe,
        ) {
            Ok(value) => value,
            Err(message)
                if message.contains("Application Control policy has blocked this file") =>
            {
                fs::remove_dir_all(&temp_root).ok();
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
        let second = run_self_host_aot_cli_with_backend(
            &mut backend_second,
            &project_dir,
            &output_exe,
        )
        .expect("second run should succeed");

        assert_eq!(first.source_file_count, second.source_file_count);
        assert_eq!(first.entry_symbol, second.entry_symbol);
        assert_eq!(first.linked_image_path, second.linked_image_path);
        assert_eq!(first.object_file_names, second.object_file_names);

        let ir_bundle_path = artifact_root.join("self_host_ir_bundle.json");
        let object_bundle_path = artifact_root.join("self_host_object_bundle.json");
        assert!(ir_bundle_path.exists(), "expected staged ir bundle metadata");
        assert!(
            object_bundle_path.exists(),
            "expected staged object bundle metadata"
        );

        fs::remove_dir_all(&temp_root).ok();
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
        let summary_stage1 =
            match run_self_host_aot_cli_with_backend(&mut backend_stage1, &project_dir, &output_stage1)
            {
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
        let summary_stage2 = run_self_host_aot_cli_with_backend(
            &mut backend_stage2,
            &project_dir,
            &output_stage2,
        )
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

        assert_eq!(summary_stage1.source_file_count, summary_stage2.source_file_count);
        assert_eq!(summary_stage1.entry_symbol, summary_stage2.entry_symbol);
        assert_eq!(summary_stage1.object_file_names, summary_stage2.object_file_names);
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
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
            Err(message) => panic!("real toolchain self-host aot-cli run should succeed: {message}"),
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
    fn self_host_aot_cli_signed_executable_smoke_if_real_toolchain_available() {
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
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
            Err(message) if message.contains("Application Control policy has blocked this file") => {
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
            Err(message) => panic!("runtime bridge live source-count build should succeed: {message}"),
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
}

fn collect_stasis_files_recursive(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("failed to read directory entry in {}: {error}", dir.display())
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
            format!("{:?}: {} ({path}:{line}:{column})", diag.severity, diag.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn host_emit_ir_from_compiler_state_with_backend(
    backend: &mut IncrementalCompilerBackend,
    project_dir: &Path,
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
        let multiple_main_error = result.diagnostics.iter().any(|diag| {
            diag.message
                .contains("multiple main declarations (code=43")
        });
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
    let strict_self_host = std::env::var("STASIS_AOT_STRICT_SELF_HOST")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    let allow_stub_fallback = std::env::var("STASIS_AOT_ALLOW_STUB_FALLBACK")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    if strict_self_host && !allow_stub_fallback && !manifest.fallback_stub_symbols.is_empty() {
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
    let quality_gate = std::env::var("STASIS_AOT_QUALITY_GATE")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    if quality_gate && manifest.fallback_stub_symbols.iter().any(|symbol| symbol == &entry_symbol) {
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
    let ir_text = std::fs::read_to_string(ir_bundle_path)
        .map_err(|error| format!("failed to read ir bundle {}: {error}", ir_bundle_path.display()))?;
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
    let (runtime_bridge_object, runtime_bridge_mode) = emit_self_host_runtime_bridge_object(backend)?;
    object_paths.push(runtime_bridge_object);
    let mut executable_entry_symbol = object_bundle.entry_symbol.clone();
    if cfg!(windows) {
        let shim_object = backend
            .aot_artifact_root
            .join("self_host_entry_shim.obj");
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
            let bridge_object = backend.aot_artifact_root.join("self_host_runtime_bridge.obj");
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
    let bridge_object = backend.aot_artifact_root.join("self_host_runtime_bridge.obj");
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
    // Runtime extern bridge stubs for self-host AOT executable linkage.
    // Real dispatch wiring will replace these stubs as lowering coverage expands.
    let bridge_clif = format!(
        "function %print_i32(i32) {cc} {{\nblock0:\nreturn\n}}\n\
function %print_string(i64) {cc} {{\nblock0:\nreturn\n}}\n\
function %host_cli_arg_count() -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 0\nreturn v0\n}}\n\
function %host_cli_arg_value(i32, i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}}\n\
function %host_set_summary_file(i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 0\nreturn v0\n}}\n\
function %host_source_file_count(i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 0\nreturn v0\n}}\n\
function %host_load_source_file(i64, i32, i64, i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}}\n\
function %host_emit_ir_from_compiler_state(i64, i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}}\n\
function %host_run_cranelift_aot(i64, i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}}\n\
function %host_link_executable_from_objects(i64, i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}}\n\
function %host_write_aot_cli_summary(i64, i64, i64) -> i32 {cc} {{\nblock0:\nv0 = iconst.i32 1\nreturn v0\n}}\n"
    );
    compile_clif_to_object(&bridge_clif, bridge_object, &backend.aot_compile_config)?;
    write_runtime_bridge_mode_marker(backend, "clif").ok();
    Ok(())
}

fn write_runtime_bridge_mode_marker(
    backend: &IncrementalCompilerBackend,
    mode: &str,
) -> Result<(), String> {
    let marker_path = backend.aot_artifact_root.join("self_host_runtime_bridge.mode");
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

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetEnvironmentVariableA(lpName: *const c_char, lpBuffer: *mut c_char, nSize: u32) -> u32;
    fn SetEnvironmentVariableA(lpName: *const c_char, lpValue: *const c_char) -> i32;
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
    let source_written = read_env_ascii(&source_key, out_source, 131072);
    if source_written == 0 {
        return 1;
    }
    0
}
#[unsafe(no_mangle)]
pub extern "system" fn host_emit_ir_from_compiler_state(_project_dir: *const u8, _out_ir_bundle: *mut u8) -> i32 { 1 }
#[unsafe(no_mangle)]
pub extern "system" fn host_run_cranelift_aot(_ir_bundle: *const u8, _out_object_bundle: *mut u8) -> i32 { 1 }
#[unsafe(no_mangle)]
pub extern "system" fn host_link_executable_from_objects(_object_bundle: *const u8, _output_exe: *const u8) -> i32 { 1 }
#[unsafe(no_mangle)]
pub extern "system" fn host_write_aot_cli_summary(_output_exe: *const u8, _ir_bundle: *const u8, _object_bundle: *const u8) -> i32 { 1 }
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
    let changed_files = collect_stasis_files_for_self_host_project(project_dir)?;
    if changed_files.is_empty() {
        return Err(format!(
            "no .stasis files found under {}",
            project_dir.display()
        ));
    }
    let ir_bundle_path = host_emit_ir_from_compiler_state_with_backend(backend, project_dir)?;
    let object_bundle_path = host_run_cranelift_aot_from_ir_bundle(&ir_bundle_path)?;
    let mut summary =
        host_link_executable_from_object_bundle_with_backend(backend, &object_bundle_path, output_exe)?;
    summary.source_file_count = changed_files.len();
    summary.ir_bundle_path = ir_bundle_path;
    summary.object_bundle_path = object_bundle_path;
    write_default_aot_cli_summary_sidecar(&summary)?;
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
        && metric.simple_i32_return_call_one_arg_target_id_hash.is_none()
        && metric
            .simple_i32_return_call_one_arg_arg_call_target_id_hash
            .is_none()
        && metric.simple_i32_return_two_call_left_target_id_hash.is_none()
        && metric.simple_i32_return_two_call_right_target_id_hash.is_none()
}

fn default_aot_cli_summary_sidecar_path(output_exe: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("STASIS_AOT_SUMMARY_FILE") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let file_name = output_exe
        .file_name()
        .map(|name| format!("{}.summary.json", name.to_string_lossy()))
        .unwrap_or_else(|| "aot_cli.summary.json".to_string());
    output_exe.with_file_name(file_name)
}

fn write_default_aot_cli_summary_sidecar(summary: &SelfHostedAotCliSummary) -> Result<(), String> {
    let sidecar_path = default_aot_cli_summary_sidecar_path(&summary.linked_image_path);
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
            .filter_map(|path| path.file_name().map(|name| name.to_string_lossy().to_string()))
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
