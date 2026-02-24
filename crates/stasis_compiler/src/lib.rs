#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalCompileOutput {
    pub status: i32,
    pub layout_hash: i32,
    pub hook_symbol: Option<String>,
    pub file_paths: Vec<String>,
    pub functions: Vec<FunctionMetric>,
    pub errors: Vec<ErrorMetric>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMetric {
    pub file_index: usize,
    pub ordinal: usize,
    pub id_hash: i32,
    pub sig_hash: i32,
    pub body_hash: i32,
    pub return_type: String,
    pub param_count: i32,
    pub first_param_type_code: i32,
    pub simple_i32_return_expr: Option<SimpleI32ReturnExpr>,
    pub simple_i32_return_call_target_id_hash: Option<i32>,
    pub simple_i32_return_call_add_delta: Option<i32>,
    pub simple_i32_return_call_one_arg_target_id_hash: Option<i32>,
    pub simple_i32_return_call_one_arg_i32_literal: Option<i32>,
    pub simple_i32_return_call_one_arg_arg_call_target_id_hash: Option<i32>,
    pub simple_i32_return_two_call_left_target_id_hash: Option<i32>,
    pub simple_i32_return_two_call_right_target_id_hash: Option<i32>,
    pub simple_i32_return_two_call_op_code: Option<i32>,
    pub simple_void_print_i32_literal: Option<i32>,
    pub simple_void_print_i32_call_target_id_hash: Option<i32>,
    pub simple_void_print_i32_call_one_arg_arg_call_target_id_hash: Option<i32>,
    pub simple_void_print_i32_call_add_delta: Option<i32>,
    pub clif_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleI32ReturnExpr {
    Literal(i32),
    Add(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Sub(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Mul(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Div(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Mod(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Select(
        SimpleI32Condition,
        Box<SimpleI32ReturnExpr>,
        Box<SimpleI32ReturnExpr>,
    ),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleI32Condition {
    Eq(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Ne(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Le(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Ge(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Lt(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    Gt(Box<SimpleI32ReturnExpr>, Box<SimpleI32ReturnExpr>),
    And(Box<SimpleI32Condition>, Box<SimpleI32Condition>),
    Or(Box<SimpleI32Condition>, Box<SimpleI32Condition>),
    Not(Box<SimpleI32Condition>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMetric {
    pub code: i32,
    pub pos: i32,
    pub detail_a: i32,
    pub detail_b: i32,
}

#[derive(Debug, Clone)]
struct FileState {
    layout_hash: i32,
    main_decl_count: i32,
    main_valid_count: i32,
    main_invalid_count: i32,
    functions: Vec<ParsedFunction>,
}

#[derive(Debug, Clone)]
struct ParsedFunction {
    ordinal: usize,
    id_hash: i32,
    sig_hash: i32,
    body_hash: i32,
    return_type: String,
    param_count: i32,
    first_param_type_code: i32,
    simple_i32_return_expr: Option<SimpleI32ReturnExpr>,
    simple_i32_return_call_target_id_hash: Option<i32>,
    simple_i32_return_call_add_delta: Option<i32>,
    simple_i32_return_call_one_arg_target_id_hash: Option<i32>,
    simple_i32_return_call_one_arg_i32_literal: Option<i32>,
    simple_i32_return_call_one_arg_arg_call_target_id_hash: Option<i32>,
    simple_i32_return_two_call_left_target_id_hash: Option<i32>,
    simple_i32_return_two_call_right_target_id_hash: Option<i32>,
    simple_i32_return_two_call_op_code: Option<i32>,
    simple_void_print_i32_literal: Option<i32>,
    simple_void_print_i32_call_target_id_hash: Option<i32>,
    simple_void_print_i32_call_one_arg_arg_call_target_id_hash: Option<i32>,
    simple_void_print_i32_call_add_delta: Option<i32>,
    call_target_id_hashes: Vec<i32>,
    clif_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FunctionKey {
    path: String,
    id_hash: i32,
    sig_hash: i32,
}

#[derive(Debug, Clone)]
struct AnalysisResult {
    functions: Vec<ParsedFunction>,
    layout_hash: i32,
    main_decl_count: i32,
    main_valid_count: i32,
    main_invalid_count: i32,
    errors: Vec<ErrorMetric>,
}

pub struct IncrementalCompilerHost {
    source_hash_by_path: BTreeMap<String, u64>,
    state_by_path: BTreeMap<String, FileState>,
    last_layout_hash_i32: i32,
    required_reachability_root_hashes: Vec<i32>,
    last_reachable_function_keys: BTreeSet<FunctionKey>,
}

impl IncrementalCompilerHost {
    pub fn new() -> Self {
        Self {
            source_hash_by_path: BTreeMap::new(),
            state_by_path: BTreeMap::new(),
            last_layout_hash_i32: 0,
            required_reachability_root_hashes: Vec::new(),
            last_reachable_function_keys: BTreeSet::new(),
        }
    }

    pub fn set_required_reachability_roots(&mut self, roots: &[&str]) {
        self.required_reachability_root_hashes.clear();
        for root in roots {
            let id_hash = hash_identifier(root);
            if !self.required_reachability_root_hashes.contains(&id_hash) {
                self.required_reachability_root_hashes.push(id_hash);
            }
        }
    }

    pub fn compile_changed_files(
        &mut self,
        changed_files: &[PathBuf],
    ) -> Result<IncrementalCompileOutput, String> {
        if changed_files.is_empty() {
            return Err("compile request had no changed files".to_string());
        }

        let mut files = changed_files.to_vec();
        files.sort();
        files.dedup();
        let previous_state_by_path = self.state_by_path.clone();

        let mut changed_sources: Vec<(String, String)> = Vec::new();
        let mut deleted_paths: Vec<String> = Vec::new();
        for path in files {
            let path_key = normalize_path_key(&path);
            match fs::read(&path) {
                Ok(bytes) => {
                    let source = String::from_utf8_lossy(&bytes).to_string();
                    let source_hash = hash_text(&source);
                    let changed = self
                        .source_hash_by_path
                        .get(&path_key)
                        .is_none_or(|existing| *existing != source_hash);
                    self.source_hash_by_path
                        .insert(path_key.clone(), source_hash);
                    if changed {
                        changed_sources.push((path_key, source));
                    }
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    let removed_hash = self.source_hash_by_path.remove(&path_key).is_some();
                    let removed_state = self.state_by_path.remove(&path_key).is_some();
                    if removed_hash || removed_state {
                        deleted_paths.push(path_key);
                    }
                }
                Err(error) => {
                    return Err(format!("failed reading {}: {error}", path.display()));
                }
            }
        }

        if changed_sources.is_empty() && deleted_paths.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 0,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths: Vec::new(),
                functions: Vec::new(),
                errors: Vec::new(),
            });
        }

        let mut file_paths = Vec::with_capacity(changed_sources.len());
        let mut functions: Vec<FunctionMetric> = Vec::new();
        let mut errors: Vec<ErrorMetric> = Vec::new();
        let mut analyzed_by_path: BTreeMap<String, AnalysisResult> = BTreeMap::new();

        for (path_key, source) in &changed_sources {
            let analyzed =
                analyze_source_via_stasis(source, &self.required_reachability_root_hashes)?;
            if !analyzed.errors.is_empty() {
                errors.extend(analyzed.errors.clone());
            }
            analyzed_by_path.insert(path_key.clone(), analyzed);
        }

        if !errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths,
                functions,
                errors,
            });
        }

        for (path_key, analyzed) in &analyzed_by_path {
            self.state_by_path.insert(
                path_key.clone(),
                FileState {
                    layout_hash: analyzed.layout_hash,
                    main_decl_count: analyzed.main_decl_count,
                    main_valid_count: analyzed.main_valid_count,
                    main_invalid_count: analyzed.main_invalid_count,
                    functions: analyzed.functions.clone(),
                },
            );
        }

        let mut main_decl_total = 0;
        let mut main_valid_total = 0;
        let mut main_invalid_total = 0;
        for state in self.state_by_path.values() {
            main_decl_total += state.main_decl_count;
            main_valid_total += state.main_valid_count;
            main_invalid_total += state.main_invalid_count;
        }

        if main_decl_total == 0 {
            errors.push(ErrorMetric {
                code: 41,
                pos: 0,
                detail_a: 0,
                detail_b: 0,
            });
        } else if main_decl_total > 1 {
            errors.push(ErrorMetric {
                code: 43,
                pos: 0,
                detail_a: main_decl_total,
                detail_b: 0,
            });
        } else if main_valid_total != 1 || main_invalid_total > 0 {
            errors.push(ErrorMetric {
                code: 42,
                pos: 0,
                detail_a: main_valid_total,
                detail_b: main_invalid_total,
            });
        }

        if !errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                hook_symbol: current_hook_symbol(&self.state_by_path),
                file_paths,
                functions,
                errors,
            });
        }

        let previous_reachable_keys = self.last_reachable_function_keys.clone();
        let current_reachable_keys = compute_reachable_function_keys_from_state(
            &self.state_by_path,
            &self.required_reachability_root_hashes,
        );
        let previous_body_hash_by_key = build_function_body_hash_by_key(&previous_state_by_path);
        let mut file_index_by_path: BTreeMap<String, usize> = BTreeMap::new();
        for (path_key, state) in &self.state_by_path {
            for parsed in &state.functions {
                let key = function_key_for(path_key, parsed);
                if !current_reachable_keys.contains(&key) {
                    continue;
                }
                let changed_definition = match previous_body_hash_by_key.get(&key) {
                    Some(previous_body_hash) => *previous_body_hash != parsed.body_hash,
                    None => true,
                };
                let previously_reachable = previous_reachable_keys.contains(&key);
                if !changed_definition && previously_reachable {
                    continue;
                }

                let file_index = if let Some(existing) = file_index_by_path.get(path_key) {
                    *existing
                } else {
                    let new_index = file_paths.len();
                    file_paths.push(path_key.clone());
                    file_index_by_path.insert(path_key.clone(), new_index);
                    new_index
                };
                functions.push(FunctionMetric {
                    file_index,
                    ordinal: parsed.ordinal,
                    id_hash: parsed.id_hash,
                    sig_hash: parsed.sig_hash,
                    body_hash: parsed.body_hash,
                    return_type: parsed.return_type.clone(),
                    param_count: parsed.param_count,
                    first_param_type_code: parsed.first_param_type_code,
                    simple_i32_return_expr: parsed.simple_i32_return_expr.clone(),
                    simple_i32_return_call_target_id_hash: parsed
                        .simple_i32_return_call_target_id_hash,
                    simple_i32_return_call_add_delta: parsed.simple_i32_return_call_add_delta,
                    simple_i32_return_call_one_arg_target_id_hash: parsed
                        .simple_i32_return_call_one_arg_target_id_hash,
                    simple_i32_return_call_one_arg_i32_literal: parsed
                        .simple_i32_return_call_one_arg_i32_literal,
                    simple_i32_return_call_one_arg_arg_call_target_id_hash: parsed
                        .simple_i32_return_call_one_arg_arg_call_target_id_hash,
                    simple_i32_return_two_call_left_target_id_hash: parsed
                        .simple_i32_return_two_call_left_target_id_hash,
                    simple_i32_return_two_call_right_target_id_hash: parsed
                        .simple_i32_return_two_call_right_target_id_hash,
                    simple_i32_return_two_call_op_code: parsed.simple_i32_return_two_call_op_code,
                    simple_void_print_i32_literal: parsed.simple_void_print_i32_literal,
                    simple_void_print_i32_call_target_id_hash: parsed
                        .simple_void_print_i32_call_target_id_hash,
                    simple_void_print_i32_call_one_arg_arg_call_target_id_hash: parsed
                        .simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
                    simple_void_print_i32_call_add_delta: parsed
                        .simple_void_print_i32_call_add_delta,
                    clif_text: parsed.clif_text.clone(),
                });
            }
        }

        let mut layout_acc = 216613626_i32;
        for state in self.state_by_path.values() {
            layout_acc = hash_mix(layout_acc, state.layout_hash);
        }
        self.last_layout_hash_i32 = layout_acc;
        self.last_reachable_function_keys = current_reachable_keys;

        Ok(IncrementalCompileOutput {
            status: 0,
            layout_hash: layout_acc,
            hook_symbol: current_hook_symbol(&self.state_by_path),
            file_paths,
            functions,
            errors,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        "stasis-orchestrated"
    }
}

impl Default for IncrementalCompilerHost {
    fn default() -> Self {
        Self::new()
    }
}

fn analyze_source_via_stasis(
    source: &str,
    required_reachability_root_hashes: &[i32],
) -> Result<AnalysisResult, String> {
    let harness_source = build_stasis_analysis_harness(source, required_reachability_root_hashes);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?
        .as_nanos();
    let repo_root = repo_root_path()?;
    let harness_dir = repo_root
        .join(".stasis_cache")
        .join("compiler_host")
        .join(format!("run_{stamp}"));
    fs::create_dir_all(&harness_dir).map_err(|error| {
        format!(
            "failed to create harness dir {}: {error}",
            harness_dir.display()
        )
    })?;
    let harness_path = harness_dir.join("analyze_source.stasis");
    let mut file = fs::File::create(&harness_path).map_err(|error| {
        format!(
            "failed to create harness {}: {error}",
            harness_path.display()
        )
    })?;
    file.write_all(harness_source.as_bytes()).map_err(|error| {
        format!(
            "failed to write harness {}: {error}",
            harness_path.display()
        )
    })?;
    drop(file);

    let cli = bootstrap_stasis_cli_exe_path()?;
    let output = run_stasis_analysis_harness(&cli, &harness_path)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("stasis harness failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_stasis_analysis_output(&stdout)
}

fn run_stasis_analysis_harness(
    cli: &Path,
    harness_path: &Path,
) -> Result<std::process::Output, String> {
    let mut command = Command::new(cli);
    command
        .arg("run")
        .arg(harness_path)
        // Bootstrap CLI `run` defaults to watch mode; host analysis harness must run once.
        .arg("--no-watch");
    command.output().map_err(|error| {
        format!(
            "failed running stasis compiler harness via {}: {error}",
            cli.display()
        )
    })
}

fn bootstrap_stasis_cli_exe_path() -> Result<PathBuf, String> {
    let repo_root = repo_root_path()?;
    if cfg!(windows) {
        let source_exe = repo_root
            .join("Stasis.Cli")
            .join("bin")
            .join("Release")
            .join("net9.0")
            .join("Stasis.Cli.exe");
        if source_exe.exists() {
            return Ok(source_exe);
        }
        let bootstrap_exe = repo_root
            .join("bootstrap")
            .join("windows")
            .join("stasis-cli")
            .join("Stasis.Cli.exe");
        if bootstrap_exe.exists() {
            Ok(bootstrap_exe)
        } else {
            Err(format!(
                "stasis cli executable not found at {}",
                bootstrap_exe.display()
            ))
        }
    } else {
        Err("stasis cli executable is currently only wired for Windows".to_string())
    }
}

fn repo_root_path() -> Result<PathBuf, String> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_root
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to resolve repo root".to_string())
}

fn build_stasis_analysis_harness(
    source: &str,
    required_reachability_root_hashes: &[i32],
) -> String {
    let mut out = String::new();
    out.push_str("import \"../../../src/stdlib/stdlib.stasis\";\n");
    out.push_str("import \"../../../compiler/simple_pass_compiler.stasis\";\n");
    out.push_str("global src_buf: ascii[262144];\n");
    out.push_str("global clif_buf: ascii[8192];\n");
    out.push_str("function load_source(): void {\n");
    out.push_str("    ascii_clear(src_buf);\n");
    for byte in source.as_bytes() {
        out.push_str("    ascii_push(src_buf, ");
        out.push_str(&byte.to_string());
        out.push_str(");\n");
    }
    out.push_str("}\n");
    out.push_str("function emit_metrics(status: i32): void {\n");
    out.push_str("    print_string(\"__SC_BEGIN;\");\n");
    out.push_str("    print_string(\"status=\"); print_i32(status); print_string(\";\");\n");
    out.push_str(
        "    print_string(\"layout=\"); print_i32(Compiler.layout_hash); print_string(\";\");\n",
    );
    out.push_str("    print_string(\"main_decl=\"); print_i32(Compiler.parsed_main_declaration_count); print_string(\";\");\n");
    out.push_str("    print_string(\"main_valid=\"); print_i32(Compiler.parsed_main_valid_i32_count); print_string(\";\");\n");
    out.push_str("    print_string(\"main_invalid=\"); print_i32(Compiler.parsed_main_invalid_signature_count); print_string(\";\");\n");
    out.push_str(
        "    print_string(\"errors=\"); print_i32(Compiler.error_count); print_string(\";\");\n",
    );
    out.push_str("    let ei: i32 = 0;\n");
    out.push_str("    for (ei = 0; ei < Compiler.error_count; ei += 1) {\n");
    out.push_str("        print_string(\"err=\");\n");
    out.push_str("        print_i32(Compiler.errors[ei].code); print_string(\",\");\n");
    out.push_str("        print_i32(Compiler.errors[ei].pos); print_string(\",\");\n");
    out.push_str("        print_i32(Compiler.errors[ei].detail_a); print_string(\",\");\n");
    out.push_str("        print_i32(Compiler.errors[ei].detail_b); print_string(\";\");\n");
    out.push_str("    }\n");
    out.push_str("    let reachable_count: i32 = 0;\n");
    out.push_str("    let fi: i32 = 0;\n");
    out.push_str("    for (fi = 0; fi < Compiler.tracked_function_count; fi += 1) {\n");
    out.push_str("        if (compiler_is_function_reachable(fi)) {\n");
    out.push_str("            reachable_count += 1;\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str(
        "    print_string(\"fn_count=\"); print_i32(reachable_count); print_string(\";\");\n",
    );
    out.push_str("    for (fi = 0; fi < Compiler.tracked_function_count; fi += 1) {\n");
    out.push_str("        print_string(\"fn=\");\n");
    out.push_str("        print_i32(fi); print_string(\",\");\n");
    out.push_str("        print_i32(Compiler.function_id_hashes[fi]); print_string(\",\");\n");
    out.push_str("        print_i32(Compiler.function_sig_hashes[fi]); print_string(\",\");\n");
    out.push_str("        print_i32(Compiler.function_body_hashes[fi]); print_string(\",\");\n");
    out.push_str(
        "        print_i32(Compiler.function_return_type_codes[fi]); print_string(\",\");\n",
    );
    out.push_str("        print_i32(Compiler.function_param_counts[fi]); print_string(\",\");\n");
    out.push_str(
        "        print_i32(Compiler.function_first_param_type_codes[fi]); print_string(\",\");\n",
    );
    out.push_str(
        "        print_i32(Compiler.function_simple_i32_return_literal_flags[fi]); print_string(\",\");\n",
    );
    out.push_str("        print_i32(Compiler.function_simple_i32_return_literals[fi]); print_string(\",\");\n");
    out.push_str("        if (compiler_is_function_reachable(fi)) { print_i32(1); } else { print_i32(0); }\n");
    out.push_str("        print_string(\";\");\n");
    out.push_str("        let edge_count: i32 = Compiler.function_call_edge_counts[fi];\n");
    out.push_str("        let edge_index: i32 = 0;\n");
    out.push_str("        for (edge_index = 0; edge_index < edge_count; edge_index += 1) {\n");
    out.push_str("            print_string(\"edge=\");\n");
    out.push_str(
        "            print_i32(Compiler.function_call_edge_hashes[fi * COMPILER_MAX_FUNCTION_CALL_EDGES + edge_index]);\n",
    );
    out.push_str("            print_string(\";\");\n");
    out.push_str("        }\n");
    out.push_str("        compiler_emit_function_clif_for_index(fi, clif_buf);\n");
    out.push_str("        print_string(\"clif=\"); print_string(clif_buf); print_string(\";\");\n");
    out.push_str("    }\n");
    out.push_str("    print_string(\"__SC_END;\");\n");
    out.push_str("}\n");
    out.push_str("function main(): i32 {\n");
    out.push_str("    compiler_reset_state();\n");
    out.push_str("    compiler_clear_required_reachability_roots();\n");
    for root_hash in required_reachability_root_hashes {
        out.push_str("    compiler_add_required_reachability_root_hash(");
        out.push_str(&root_hash.to_string());
        out.push_str(");\n");
    }
    if cfg!(windows) {
        out.push_str("    Compiler.clif_call_conv_code = 1;\n");
    } else {
        out.push_str("    Compiler.clif_call_conv_code = 2;\n");
    }
    out.push_str("    load_source();\n");
    out.push_str("    compiler_set_source(src_buf);\n");
    out.push_str("    let status: i32 = run_incremental_compiler();\n");
    out.push_str("    emit_metrics(status);\n");
    out.push_str("    return 0;\n");
    out.push_str("}\n");
    out
}

fn parse_stasis_analysis_output(stdout: &str) -> Result<AnalysisResult, String> {
    fn parse_i32(value: &str) -> i32 {
        value.trim().parse::<i32>().unwrap_or_default()
    }
    fn parse_usize(value: &str) -> usize {
        value.trim().parse::<usize>().unwrap_or_default()
    }

    let begin = stdout
        .find("__SC_BEGIN;")
        .ok_or_else(|| format!("missing harness begin marker in output: {stdout}"))?;
    let end = stdout[begin..]
        .find("__SC_END;")
        .map(|index| begin + index)
        .ok_or_else(|| format!("missing harness end marker in output: {stdout}"))?;
    let payload = &stdout[begin + "__SC_BEGIN;".len()..end];

    let mut status = 0i32;
    let mut layout_hash = 0i32;
    let mut main_decl_count = 0i32;
    let mut main_valid_count = 0i32;
    let mut main_invalid_count = 0i32;
    let mut errors: Vec<ErrorMetric> = Vec::new();
    let mut functions: Vec<ParsedFunction> = Vec::new();
    for token in payload.split(';') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(value) = token.strip_prefix("status=") {
            status = parse_i32(value);
            continue;
        }
        if let Some(value) = token.strip_prefix("layout=") {
            layout_hash = parse_i32(value);
            continue;
        }
        if let Some(value) = token.strip_prefix("main_decl=") {
            main_decl_count = parse_i32(value);
            continue;
        }
        if let Some(value) = token.strip_prefix("main_valid=") {
            main_valid_count = parse_i32(value);
            continue;
        }
        if let Some(value) = token.strip_prefix("main_invalid=") {
            main_invalid_count = parse_i32(value);
            continue;
        }
        if let Some(value) = token.strip_prefix("err=") {
            let parts: Vec<&str> = value.split(',').collect();
            if parts.len() == 4 {
                errors.push(ErrorMetric {
                    code: parse_i32(parts[0]),
                    pos: parse_i32(parts[1]),
                    detail_a: parse_i32(parts[2]),
                    detail_b: parse_i32(parts[3]),
                });
            }
            continue;
        }
        if let Some(value) = token.strip_prefix("fn=") {
            let parts: Vec<&str> = value.split(',').collect();
            if parts.len() >= 7 {
                let return_type = match parse_i32(parts[4]) {
                    1 => "i32".to_string(),
                    2 => "void".to_string(),
                    _ => "unknown".to_string(),
                };
                let param_count = parse_i32(parts[5]);
                let first_param_type_code = parse_i32(parts[6]);
                let simple_i32_return_literal_flag = if parts.len() >= 8 {
                    parse_i32(parts[7])
                } else {
                    0
                };
                let simple_i32_return_literal_value = if parts.len() >= 9 {
                    parse_i32(parts[8])
                } else {
                    0
                };
                let simple_i32_return_expr = if simple_i32_return_literal_flag != 0 {
                    Some(SimpleI32ReturnExpr::Literal(
                        simple_i32_return_literal_value,
                    ))
                } else {
                    None
                };
                functions.push(ParsedFunction {
                    ordinal: parse_usize(parts[0]),
                    id_hash: parse_i32(parts[1]),
                    sig_hash: parse_i32(parts[2]),
                    body_hash: parse_i32(parts[3]),
                    return_type,
                    param_count,
                    first_param_type_code,
                    simple_i32_return_expr,
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
                    call_target_id_hashes: Vec::new(),
                    clif_text: String::new(),
                });
            }
            continue;
        }
        if let Some(value) = token.strip_prefix("edge=") {
            if let Some(function) = functions.last_mut() {
                function.call_target_id_hashes.push(parse_i32(value));
            }
            continue;
        }
        if let Some(value) = token.strip_prefix("clif=") {
            if let Some(function) = functions.last_mut() {
                function.clif_text = value.to_string();
            }
            continue;
        }
    }
    if status != 0 && errors.is_empty() {
        return Err(format!(
            "stasis harness reported status {status} without diagnostics: {stdout}"
        ));
    }
    Ok(AnalysisResult {
        functions,
        layout_hash,
        main_decl_count,
        main_valid_count,
        main_invalid_count,
        errors,
    })
}

fn normalize_path_key(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut text = absolute.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    text
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn hash_mix(hash: i32, value: i32) -> i32 {
    hash.wrapping_mul(16777619)
        .wrapping_add(value.wrapping_add(1))
}

fn hash_i32(value: &str) -> i32 {
    let mut hash: i32 = 216613626;
    for byte in value.bytes() {
        hash = hash_mix(hash, i32::from(byte));
    }
    hash
}

fn hash_identifier(name: &str) -> i32 {
    hash_i32(name)
}

fn function_key_for(path: &str, function: &ParsedFunction) -> FunctionKey {
    FunctionKey {
        path: path.to_string(),
        id_hash: function.id_hash,
        sig_hash: function.sig_hash,
    }
}

fn build_function_body_hash_by_key(
    state_by_path: &BTreeMap<String, FileState>,
) -> BTreeMap<FunctionKey, i32> {
    let mut by_key = BTreeMap::new();
    for (path, state) in state_by_path {
        for function in &state.functions {
            by_key.insert(function_key_for(path, function), function.body_hash);
        }
    }
    by_key
}

fn all_reachability_root_hashes(required_roots: &[i32]) -> Vec<i32> {
    let mut roots = vec![
        hash_identifier("main"),
        hash_identifier("tick"),
        hash_identifier("on_code_swap"),
    ];
    for root in required_roots {
        if !roots.contains(root) {
            roots.push(*root);
        }
    }
    roots
}

fn compute_reachable_function_keys_from_state(
    state_by_path: &BTreeMap<String, FileState>,
    required_roots: &[i32],
) -> BTreeSet<FunctionKey> {
    let mut all_keys = Vec::new();
    let mut by_id_hash: BTreeMap<i32, Vec<FunctionKey>> = BTreeMap::new();
    let mut call_edges_by_key: BTreeMap<FunctionKey, Vec<i32>> = BTreeMap::new();

    for (path, state) in state_by_path {
        for function in &state.functions {
            let key = function_key_for(path, function);
            all_keys.push(key.clone());
            by_id_hash
                .entry(function.id_hash)
                .or_default()
                .push(key.clone());
            call_edges_by_key.insert(key, function.call_target_id_hashes.clone());
        }
    }

    if all_keys.is_empty() {
        return BTreeSet::new();
    }

    let roots = all_reachability_root_hashes(required_roots);
    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::new();
    let mut found_root = false;

    for root_hash in roots {
        if let Some(keys) = by_id_hash.get(&root_hash) {
            found_root = true;
            for key in keys {
                if reachable.insert(key.clone()) {
                    queue.push_back(key.clone());
                }
            }
        }
    }

    if !found_root {
        return all_keys.into_iter().collect();
    }

    while let Some(current) = queue.pop_front() {
        if let Some(callee_hashes) = call_edges_by_key.get(&current) {
            for callee_hash in callee_hashes {
                if let Some(callee_keys) = by_id_hash.get(callee_hash) {
                    for callee_key in callee_keys {
                        if reachable.insert(callee_key.clone()) {
                            queue.push_back(callee_key.clone());
                        }
                    }
                }
            }
        }
    }

    reachable
}

fn current_hook_symbol(state_by_path: &BTreeMap<String, FileState>) -> Option<String> {
    let hook_hash = hash_identifier("on_code_swap");
    for state in state_by_path.values() {
        if state.functions.iter().any(|func| func.id_hash == hook_hash) {
            return Some("on_code_swap".to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_file_state(functions: Vec<ParsedFunction>) -> FileState {
        FileState {
            layout_hash: 0,
            main_decl_count: 0,
            main_valid_count: 0,
            main_invalid_count: 0,
            functions,
        }
    }

    fn test_parsed_function(name: &str, sig_hash: i32, callees: &[&str]) -> ParsedFunction {
        ParsedFunction {
            ordinal: 0,
            id_hash: hash_identifier(name),
            sig_hash,
            body_hash: sig_hash.wrapping_mul(31),
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
            call_target_id_hashes: callees
                .iter()
                .map(|callee| hash_identifier(callee))
                .collect(),
            clif_text: String::new(),
        }
    }

    #[test]
    fn backend_name_is_stasis_orchestrated() {
        let host = IncrementalCompilerHost::new();
        assert_eq!(host.backend_name(), "stasis-orchestrated");
    }

    #[test]
    fn compile_empty_change_set_is_error() {
        let mut host = IncrementalCompilerHost::new();
        let err = host.compile_changed_files(&[]).expect_err("expected error");
        assert!(err.contains("no changed files"));
    }

    #[test]
    fn in_memory_reachability_is_transitive_from_default_roots() {
        let path_a = "/tmp/a.stasis".to_string();
        let path_b = "/tmp/b.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        state_by_path.insert(
            path_a.clone(),
            test_file_state(vec![
                test_parsed_function("main", 11, &["bridge"]),
                test_parsed_function("dead", 12, &[]),
            ]),
        );
        state_by_path.insert(
            path_b.clone(),
            test_file_state(vec![
                test_parsed_function("bridge", 21, &["leaf"]),
                test_parsed_function("leaf", 22, &[]),
            ]),
        );

        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &[]);
        assert!(reachable.contains(&FunctionKey {
            path: path_a.clone(),
            id_hash: hash_identifier("main"),
            sig_hash: 11,
        }));
        assert!(reachable.contains(&FunctionKey {
            path: path_b.clone(),
            id_hash: hash_identifier("bridge"),
            sig_hash: 21,
        }));
        assert!(reachable.contains(&FunctionKey {
            path: path_b,
            id_hash: hash_identifier("leaf"),
            sig_hash: 22,
        }));
        assert!(!reachable.contains(&FunctionKey {
            path: path_a,
            id_hash: hash_identifier("dead"),
            sig_hash: 12,
        }));
    }

    #[test]
    fn in_memory_reachability_keeps_all_when_no_roots_exist() {
        let path = "/tmp/helpers.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        state_by_path.insert(
            path.clone(),
            test_file_state(vec![
                test_parsed_function("helper_a", 31, &[]),
                test_parsed_function("helper_b", 32, &["helper_a"]),
            ]),
        );

        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &[]);
        assert_eq!(reachable.len(), 2);
        assert!(reachable.contains(&FunctionKey {
            path: path.clone(),
            id_hash: hash_identifier("helper_a"),
            sig_hash: 31,
        }));
        assert!(reachable.contains(&FunctionKey {
            path,
            id_hash: hash_identifier("helper_b"),
            sig_hash: 32,
        }));
    }

    #[test]
    fn in_memory_reachability_honors_required_roots() {
        let path = "/tmp/required_root.stasis".to_string();
        let mut state_by_path = BTreeMap::new();
        state_by_path.insert(
            path.clone(),
            test_file_state(vec![
                test_parsed_function("main", 41, &[]),
                test_parsed_function("bridge_entry", 42, &[]),
            ]),
        );

        let required = [hash_identifier("bridge_entry")];
        let reachable = compute_reachable_function_keys_from_state(&state_by_path, &required);
        assert!(reachable.contains(&FunctionKey {
            path: path.clone(),
            id_hash: hash_identifier("main"),
            sig_hash: 41,
        }));
        assert!(reachable.contains(&FunctionKey {
            path,
            id_hash: hash_identifier("bridge_entry"),
            sig_hash: 42,
        }));
    }

    #[test]
    fn compile_deleted_file_updates_state_without_read_error() {
        let temp = std::env::temp_dir().join(format!(
            "stasis_compiler_deleted_file_{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&temp);
        let source_path = temp.join("main.stasis");
        fs::write(&source_path, "function main(): i32 { return 0; }").expect("write source");

        let mut host = IncrementalCompilerHost::new();
        let first = host
            .compile_changed_files(std::slice::from_ref(&source_path))
            .expect("first compile should succeed");
        assert_eq!(first.status, 0);

        fs::remove_file(&source_path).expect("remove source");
        let deleted = host
            .compile_changed_files(std::slice::from_ref(&source_path))
            .expect("deleted file should not return read error");
        assert_eq!(deleted.status, 2);
        assert!(deleted.errors.iter().any(|error| error.code == 41));
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn harness_run_invocation_disables_watch_mode() {
        let source = include_str!("lib.rs");
        assert!(
            source.contains(".arg(\"run\")"),
            "expected harness to invoke stasis cli run mode"
        );
        assert!(
            source.contains(".arg(\"--no-watch\")"),
            "expected harness to force single-shot mode"
        );
    }

    #[test]
    fn harness_no_longer_uses_bootstrap_wrapper_preprocess_path() {
        let path = bootstrap_stasis_cli_exe_path().expect("stasis cli path");
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        assert!(
            name.eq_ignore_ascii_case("Stasis.Cli.exe"),
            "expected host harness to resolve stasis cli executable, got {}",
            path.display()
        );
    }

    #[test]
    fn harness_source_buffer_matches_compiler_max_source_budget() {
        let harness = build_stasis_analysis_harness("function main(): i32 { return 0; }\n", &[]);
        assert!(
            harness.contains("global src_buf: ascii[262144];"),
            "expected harness source buffer to match expanded compiler source budget"
        );
    }

    #[test]
    fn compile_records_function_hashes_return_type_and_hook_symbol() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_metrics_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.hook_symbol.as_deref(), Some("on_code_swap"));
        assert_eq!(compile.functions.len(), 2);
        assert!(compile.functions.iter().any(|f| f.return_type == "i32"));
        assert!(compile.functions.iter().any(|f| f.return_type == "void"));
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(
            main.simple_i32_return_expr,
            Some(SimpleI32ReturnExpr::Literal(0))
        );
        assert_eq!(hook.simple_i32_return_expr, None);
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_target_id_hash.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_add_delta.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_one_arg_target_id_hash.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_i32_return_call_one_arg_i32_literal.is_none()));
        assert!(compile.functions.iter().all(|f| f
            .simple_i32_return_call_one_arg_arg_call_target_id_hash
            .is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_void_print_i32_literal.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_void_print_i32_call_target_id_hash.is_none()));
        assert!(compile
            .functions
            .iter()
            .all(|f| f.simple_void_print_i32_call_add_delta.is_none()));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_target_hash() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_call_target_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(): i32 { return 7; }\nfunction main(): i32 { return callee(); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        assert_eq!(main.simple_i32_return_call_one_arg_target_id_hash, None);
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_add_delta() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_call_target_add_delta_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(): i32 { return 7; }\nfunction main(): i32 { return callee() + 5; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(5));
        assert_eq!(main.simple_i32_return_call_one_arg_target_id_hash, None);
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_call_target_one_arg_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(main.simple_i32_return_call_target_id_hash, None);
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(9));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        let callee = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("callee"))
            .expect("callee metric");
        assert_eq!(callee.param_count, 1);
        assert_eq!(callee.first_param_type_code, 1);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_first_param_passthrough_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_set_summary_file(summary_file: ascii[]): i32;\nfunction forward(summary_file: ascii[]): i32 { return host_set_summary_file(summary_file); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_set_summary_file"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_first_second_param_passthrough_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_two_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(index: i32, out_value: ascii[]): i32 { return host_cli_arg_value(index, out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 2);
        assert_eq!(forward.first_param_type_code, 1);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_first_second_third_param_passthrough_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_three_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_write_aot_cli_summary(output_exe: ascii[], ir_bundle_path: ascii[], object_bundle_path: ascii[]): i32;\nfunction forward(output_exe: ascii[], ir_bundle_path: ascii[], object_bundle_path: ascii[]): i32 { return host_write_aot_cli_summary(output_exe, ir_bundle_path, object_bundle_path); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 3);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_write_aot_cli_summary"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_first_second_third_fourth_param_passthrough_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_four_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_load_source_file(project_dir: ascii[], file_index: i32, out_path: ascii[], out_source: ascii[]): i32;\nfunction forward(project_dir: ascii[], file_index: i32, out_path: ascii[], out_source: ascii[]): i32 { return host_load_source_file(project_dir, file_index, out_path, out_source); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 4);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_load_source_file"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_first_second_third_fourth_param_passthrough_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_four_param_passthrough_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_load_source_file(project_dir: ascii[], file_index: i32, out_path: ascii[], out_source: ascii[]): i32;\nfunction forward(project_dir: ascii[], file_index: i32, out_path: ascii[], out_source: ascii[]): i32 { return host_load_source_file(project_dir, file_index, out_path, out_source) + 2; }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 4);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_load_source_file"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, Some(2));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_first_second_param_passthrough_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_literal_first_second_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value(1, out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(1));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, None);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_first_second_param_passthrough_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_literal_first_second_param_passthrough_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value(1, out_value) - 2; }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(1));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, Some(-2));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_expression_first_second_param_passthrough_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_lit_expr_first_second_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value(1 + 2, out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(3));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, None);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_expression_first_second_param_passthrough_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_lit_expr_first_second_param_passthrough_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value(1 + 2, out_value) - 4; }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(3));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, Some(-4));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_first_second_param_passthrough_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_paren_lit_expr_first_second_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1 + 2), out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(3));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, None);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_first_second_param_passthrough_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_paren_lit_expr_first_second_param_passthrough_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1 + 2), out_value) - 4; }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(3));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, Some(-4));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_parenthesized_literal_first_second_param_passthrough_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_paren_lit_first_second_param_passthrough_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1), out_value); }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(1));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, None);

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_parenthesized_literal_first_second_param_passthrough_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_paren_lit_first_second_param_passthrough_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "extern function host_cli_arg_value(index: i32, out_value: ascii[]): i32;\nfunction forward(out_value: ascii[]): i32 { return host_cli_arg_value((1), out_value) - 4; }\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let forward = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("forward"))
            .expect("forward metric");
        assert_eq!(forward.param_count, 1);
        assert_eq!(forward.first_param_type_code, 0);
        assert_eq!(
            forward.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("host_cli_arg_value"))
        );
        assert_eq!(forward.simple_i32_return_call_one_arg_i32_literal, Some(1));
        assert_eq!(
            forward.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(forward.simple_i32_return_call_add_delta, Some(-4));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_add_delta() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_call_target_one_arg_lit_add_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9) + 2; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(9));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(2));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_expression_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_call_target_one_arg_lit_expr_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9 + 2); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(11));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_expression_add_delta_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_lit_expr_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9 - 2) + 5; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(7));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(5));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_paren_lit_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee((9 + 2)); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(11));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_parenthesized_literal_expression_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_paren_lit_expr_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee((9 - 2)) + 5; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(7));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(5));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_multiply_expression_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_lit_mul_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(3 * 7); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(21));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_multiply_expression_add_delta_metadata(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_lit_mul_expr_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(3 * 7) - 4; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(21));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(-4));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_divide_expression_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_lit_div_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(8 / 2); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(4));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_literal_mod_expression_add_delta_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_lit_mod_expr_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9 % 4) + 1; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, Some(1));
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(1));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_does_not_fold_simple_i32_return_call_one_arg_literal_divide_by_zero_expression() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_call_target_one_arg_lit_div_zero_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(9 / 0); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 2);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(main.simple_i32_return_call_one_arg_target_id_hash, None);
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_call_arg_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_call_target_one_arg_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function arg_fn(): i32 { return 3; }\nfunction callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(arg_fn()); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 3);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            Some(hash_identifier("arg_fn"))
        );
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_call_one_arg_call_arg_add_delta() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_call_target_one_arg_call_add_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function arg_fn(): i32 { return 3; }\nfunction callee(value: i32): i32 { return value; }\nfunction main(): i32 { return callee(arg_fn()) - 4; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 3);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(
            main.simple_i32_return_call_one_arg_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            Some(hash_identifier("arg_fn"))
        );
        assert_eq!(main.simple_i32_return_call_add_delta, Some(-4));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_i32_return_two_call_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_two_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function lhs(): i32 { return 7; }\nfunction rhs(): i32 { return 2; }\nfunction main(): i32 { return lhs() - rhs(); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        assert_eq!(compile.functions.len(), 3);
        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert_eq!(main.simple_i32_return_call_target_id_hash, None);
        assert_eq!(main.simple_i32_return_call_add_delta, None);
        assert_eq!(main.simple_i32_return_call_one_arg_target_id_hash, None);
        assert_eq!(main.simple_i32_return_call_one_arg_i32_literal, None);
        assert_eq!(
            main.simple_i32_return_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(
            main.simple_i32_return_two_call_left_target_id_hash,
            Some(hash_identifier("lhs"))
        );
        assert_eq!(
            main.simple_i32_return_two_call_right_target_id_hash,
            Some(hash_identifier("rhs"))
        );
        assert_eq!(main.simple_i32_return_two_call_op_code, Some(2));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_literal() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_void_print_i32_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { print_i32(77); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(77));
        assert_eq!(hook.simple_void_print_i32_call_target_id_hash, None);
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_call_target_hash() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_void_print_i32_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(): i32 { return 9; }\nfunction on_code_swap(): void { print_i32(callee()); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_literal_add_expression() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_void_print_i32_literal_add_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { print_i32(7 + 5); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(12));
        assert_eq!(hook.simple_void_print_i32_call_target_id_hash, None);
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_call_target_add_delta() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_void_print_i32_call_add_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(): i32 { return 9; }\nfunction on_code_swap(): void { print_i32(callee() - 4); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, Some(-4));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_and_literal() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_void_print_i32_call_one_arg_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(13)); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(13));
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_with_literal_multiply_expression()
    {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_lit_mul_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(2 * 3)); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(6));
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_with_literal_divide_expression_add_delta(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_lit_div_expr_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(8 / 2) - 1); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(4));
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, Some(-1));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_with_literal_mod_expression_add_delta(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_lit_mod_expr_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(9 % 4) + 2); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(1));
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, Some(2));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_does_not_fold_simple_void_print_i32_one_arg_call_target_literal_divide_by_zero_expression(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_lit_div_zero_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(9 / 0)); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(hook.simple_void_print_i32_call_target_id_hash, None);
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_does_not_fold_simple_void_print_i32_one_arg_call_target_literal_mod_by_zero_expression(
    ) {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_lit_mod_zero_expr_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(9 % 0)); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(hook.simple_void_print_i32_call_target_id_hash, None);
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            None
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_with_arg_call() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_call_arg_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction arg_fn(): i32 { return 6; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(arg_fn())); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            Some(hash_identifier("arg_fn"))
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_literal_add_delta() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_lit_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(13) + 2); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, Some(13));
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, Some(2));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_one_arg_call_target_with_arg_call_add_delta() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!(
            "stasis_inc_void_print_i32_call_one_arg_call_add_{stamp}"
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction arg_fn(): i32 { return 6; }\nfunction callee(x: i32): i32 { return x; }\nfunction on_code_swap(): void { print_i32(callee(arg_fn()) - 4); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(
            hook.simple_void_print_i32_call_target_id_hash,
            Some(hash_identifier("callee"))
        );
        assert_eq!(
            hook.simple_void_print_i32_call_one_arg_arg_call_target_id_hash,
            Some(hash_identifier("arg_fn"))
        );
        assert_eq!(hook.simple_void_print_i32_call_add_delta, Some(-4));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_records_simple_void_print_i32_two_call_metadata() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_void_print_i32_two_call_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction lhs(): i32 { return 7; }\nfunction rhs(): i32 { return 2; }\nfunction on_code_swap(): void { print_i32(lhs() - rhs()); return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 0);
        let hook = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("on_code_swap"))
            .expect("hook metric");
        assert_eq!(hook.simple_void_print_i32_literal, None);
        assert_eq!(hook.simple_void_print_i32_call_target_id_hash, None);
        assert_eq!(
            hook.simple_i32_return_two_call_left_target_id_hash,
            Some(hash_identifier("lhs"))
        );
        assert_eq!(
            hook.simple_i32_return_two_call_right_target_id_hash,
            Some(hash_identifier("rhs"))
        );
        assert_eq!(hook.simple_i32_return_two_call_op_code, Some(2));
        assert_eq!(hook.simple_void_print_i32_call_add_delta, None);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn second_compile_without_source_change_emits_no_functions() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_no_change_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { return 0; }\nfunction tick(): void { return; }\n",
        )
        .expect("write sample");

        let first = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert!(first.functions.len() >= 2);

        let second = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert_eq!(second.functions.len(), 0);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn compile_emits_only_changed_functions_after_edit() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_changed_fn_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let baseline = "function main(): i32 { return 0; }\nfunction tick(): void { return; }\n";
        fs::write(&file, baseline).expect("write baseline");

        let first = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert_eq!(first.functions.len(), 2);

        let updated = "function main(): i32 { return 1; }\nfunction tick(): void { return; }\n";
        fs::write(&file, updated).expect("write updated");
        let second = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert_eq!(second.functions.len(), 1);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn reachability_prunes_unreachable_helper_functions_from_emission() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_reachability_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function helper(): i32 { return 9; }\nfunction tick(): void { return; }\nfunction main(): i32 { return 1; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("tick")));
        assert!(!compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("helper")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn host_required_reachability_root_keeps_otherwise_unreachable_function() {
        let mut host = IncrementalCompilerHost::new();
        host.set_required_reachability_roots(&["bridge_entry"]);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_required_root_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function bridge_entry(): i32 { return 9; }\nfunction main(): i32 { return 1; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(compile
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("bridge_entry")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn cross_file_reachability_emits_newly_reachable_unchanged_callee() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_cross_file_reachability_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file_a = temp_root.join("a.stasis");
        let file_b = temp_root.join("b.stasis");
        fs::write(&file_a, "function main(): i32 { return 0; }\n").expect("write a baseline");
        fs::write(
            &file_b,
            "function helper(): i32 { return 7; }\nfunction dead(): i32 { return 0; }\n",
        )
        .expect("write b baseline");

        let first = host
            .compile_changed_files(&[file_a.clone(), file_b.clone()])
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert!(first
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(!first
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("helper")));

        fs::write(&file_a, "function main(): i32 { return helper(); }\n").expect("write a updated");
        let second = host
            .compile_changed_files(std::slice::from_ref(&file_a))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert!(second
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("main")));
        assert!(second
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("helper")));
        assert!(!second
            .functions
            .iter()
            .any(|f| f.id_hash == hash_identifier("dead")));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn receiver_overloads_produce_distinct_signature_hashes() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_receiver_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "function main(): i32 { damage(0, 1); return 0; }\nfunction damage(self: Enemy, amount: i32): void { return; }\nfunction damage(self: Hero, amount: i32): void { return; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        let damage = compile
            .functions
            .iter()
            .filter(|f| f.id_hash == hash_identifier("damage"))
            .collect::<Vec<_>>();
        assert_eq!(damage.len(), 2);
        assert_ne!(damage[0].sig_hash, damage[1].sig_hash);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn struct_global_lowering_uses_single_owner_global_and_import_for_secondary_function() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_global_owner_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "struct Enemy { hp: i32; }\n\
             global State { score: i32; enemy: Enemy; }\n\
             function set_first(): i32 { State.enemy.hp = 3; return State.enemy.hp; }\n\
             function set_second(): i32 { State.score = 7; return State.score; }\n\
             function main(): i32 { set_first(); return set_second(); }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);

        let first = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("set_first"))
            .expect("set_first metric");
        let second = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("set_second"))
            .expect("set_second metric");

        assert!(
            first.clif_text.contains("global sp_global_mem_layout_"),
            "expected owner function to define global arena: {}",
            first.clif_text
        );
        assert!(
            !first
                .clif_text
                .contains("global_import sp_global_mem_layout_"),
            "owner function should not import arena: {}",
            first.clif_text
        );
        assert!(
            second
                .clif_text
                .contains("global_import sp_global_mem_layout_"),
            "secondary function should import arena: {}",
            second.clif_text
        );
        assert!(
            !second.clif_text.contains("global sp_global_mem_layout_"),
            "secondary function should not redefine arena: {}",
            second.clif_text
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn reachability_prunes_unreachable_struct_and_global_layout_from_emitted_arena() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_inc_dead_struct_global_prune_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        fs::write(
            &file,
            "struct Live { hp: i32; }\n\
             struct Dead { value: i32; }\n\
             global LiveState { enemy: Live; }\n\
             global DeadState { dead: Dead; }\n\
             function dead_write(): i32 { DeadState.dead.value = 9; return DeadState.dead.value; }\n\
             function main(): i32 { LiveState.enemy.hp = 7; return LiveState.enemy.hp; }\n",
        )
        .expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile");
        assert_eq!(compile.status, 0);
        assert!(compile
            .functions
            .iter()
            .any(|function| function.id_hash == hash_identifier("main")));
        assert!(!compile
            .functions
            .iter()
            .any(|function| function.id_hash == hash_identifier("dead_write")));

        let main = compile
            .functions
            .iter()
            .find(|function| function.id_hash == hash_identifier("main"))
            .expect("main metric");
        assert!(
            main.clif_text.contains("global sp_global_mem_layout_"),
            "expected main to own shared arena: {}",
            main.clif_text
        );
        assert!(
            main.clif_text.contains(": i8[4]"),
            "expected arena size to include only reachable field: {}",
            main.clif_text
        );
        assert!(
            !main.clif_text.contains(": i8[8]"),
            "unexpected arena size indicates dead layout survived pruning: {}",
            main.clif_text
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn from_conversion_in_expression_is_semantic_error() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_from_expr_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let source =
            "function main(): i32 { let x: i32 = 0; let y: i32 = 1; let z: i32 = x.from_i32(y); return 0; }\n";
        fs::write(&file, source).expect("write sample");

        let compile = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("compile result");
        assert_eq!(compile.status, 2);
        assert!(compile.errors.iter().any(|error| error.code == 4001));
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn hook_symbol_persists_when_non_hook_function_changes() {
        let mut host = IncrementalCompilerHost::new();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_inc_hook_symbol_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let file = temp_root.join("sample.stasis");
        let baseline =
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }\n";
        fs::write(&file, baseline).expect("write baseline");

        let first = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("first compile");
        assert_eq!(first.status, 0);
        assert_eq!(first.hook_symbol.as_deref(), Some("on_code_swap"));

        let updated =
            "function main(): i32 { return 1; }\nfunction on_code_swap(): void { return; }\n";
        fs::write(&file, updated).expect("write updated");
        let second = host
            .compile_changed_files(std::slice::from_ref(&file))
            .expect("second compile");
        assert_eq!(second.status, 0);
        assert_eq!(second.hook_symbol.as_deref(), Some("on_code_swap"));
        assert_eq!(second.functions.len(), 1);
        assert_eq!(
            second.functions[0].simple_i32_return_expr,
            Some(SimpleI32ReturnExpr::Literal(1))
        );
        fs::remove_dir_all(&temp_root).ok();
    }
}
